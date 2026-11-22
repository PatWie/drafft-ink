//! The task of one sealed room. It owns the [`Relay`] core, feeds it what connections send and the time, and carries
//! out what it returns. The core calls blocking stores (files, SQLite), so each call runs on the blocking pool.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use drafftink_protocol::v2::{ImageName, Proof, RoomId};
use drafftink_relay::{
    ConnId, Effect, Frame, ImageOp, ImageRequest, ImageResponse, ImageStatus, Output, Relay, RelayConfig,
};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tracing::{debug, warn};

use super::SealedHost;
use super::storage::RoomStores;
use crate::clock::now_ms;

/// Depth of a room's command channel. A connection waits when it is full, which slows only that sender.
pub(super) const COMMAND_QUEUE: usize = 1024;
/// Depth of a connection's queue of messages to write. A reader that falls this far behind is closed: it rejoins
/// with its last sequence number and catches up, which costs less than holding what it has not read.
pub(super) const OUTBOUND_QUEUE: usize = 512;

/// What a room task tells a connection's writer.
pub(super) enum Outbound {
    /// A binary frame.
    Binary(Frame),
    /// A text frame (the keepalive `pong`).
    Text(&'static str),
    /// Close the socket.
    Close,
}

/// An image request that can cross a channel.
pub(super) struct ImageJob {
    pub(super) proof: Proof,
    pub(super) op: OwnedImageOp,
}

pub(super) enum OwnedImageOp {
    Put { name: ImageName, body: Vec<u8> },
    Get { name: ImageName },
    List,
}

/// A command from a connection to its room.
pub(super) enum Cmd {
    Connect { conn: ConnId, out: mpsc::Sender<Outbound> },
    Frame { conn: ConnId, bytes: Vec<u8> },
    Text { conn: ConnId, text: String },
    Closed { conn: ConnId },
    Image { job: ImageJob, reply: oneshot::Sender<ImageResponse> },
    Shutdown { done: oneshot::Sender<()> },
}

/// A handle to a running room.
#[derive(Clone)]
pub(super) struct RoomHandle {
    pub(super) tx: mpsc::Sender<Cmd>,
    pub(super) generation: u64,
}

type SealedRelay = Relay<Box<dyn drafftink_relay::RelayStore + Send>, Box<dyn drafftink_relay::BlobStore + Send>>;

struct Actor {
    host: Arc<SealedHost>,
    room: RoomId,
    relay: Option<SealedRelay>,
    outs: HashMap<ConnId, mpsc::Sender<Outbound>>,
    wake_at: Option<u64>,
    deleted: bool,
    idle_since: Instant,
}

/// A short tag for logs. A room id is part of the address and not a secret, but a log line does not need all of it.
fn short(room: &RoomId) -> String {
    room.to_string().chars().take(6).collect()
}

/// Starts the task of a room.
pub(super) fn spawn(
    host: Arc<SealedHost>,
    room: RoomId,
    stores: RoomStores,
    config: RelayConfig,
    generation: u64,
) -> RoomHandle {
    let (tx, rx) = mpsc::channel(COMMAND_QUEUE);
    let relay = Relay::new(room, config, stores.0, stores.1);
    let actor = Actor {
        host,
        room,
        relay: Some(relay),
        outs: HashMap::new(),
        wake_at: None,
        deleted: false,
        idle_since: Instant::now(),
    };
    tokio::spawn(actor.run(rx, generation));
    RoomHandle { tx, generation }
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::Receiver<Cmd>, generation: u64) {
        // A stored room may be past its deadline already, so look at the clock once.
        self.call(|relay, now| relay.on_wake(now)).await;
        let idle_limit = self.host.config.timing.room_idle;
        loop {
            if self.relay.is_none() || self.deleted {
                break;
            }
            let timer = self.wake_at.map(|at| Duration::from_millis(at.saturating_sub(now_ms())));
            let idle = (self.outs.is_empty() && self.wake_at.is_none()).then(|| self.idle_since + idle_limit);
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    None => break,
                    Some(Cmd::Shutdown { done }) => {
                        self.call(|relay, now| relay.shutdown(now)).await;
                        let _ = done.send(());
                        break;
                    }
                    Some(cmd) => self.handle(cmd).await,
                },
                () = async { tokio::time::sleep(timer.unwrap_or_default()).await }, if timer.is_some() => {
                    self.call(|relay, now| relay.on_wake(now)).await;
                }
                () = async { tokio::time::sleep_until(idle.unwrap_or_else(Instant::now)).await }, if idle.is_some() => break,
            }
        }
        // Unregister first, so that a connection that arrives now starts a fresh task.
        self.host.forget(&self.room, generation).await;
        rx.close();
        for out in self.outs.values() {
            let _ = out.try_send(Outbound::Close);
        }
        debug!("sealed room {} unloaded", short(&self.room));
    }

    async fn handle(&mut self, cmd: Cmd) {
        self.idle_since = Instant::now();
        match cmd {
            Cmd::Connect { conn, out } => {
                self.outs.insert(conn, out);
                self.call(move |relay, now| relay.connect(conn, now)).await;
            }
            Cmd::Frame { conn, bytes } => {
                self.call(move |relay, now| relay.on_frame(conn, now, &bytes)).await;
            }
            Cmd::Text { conn, text } => {
                self.call(move |relay, now| relay.on_text(conn, now, &text)).await;
            }
            Cmd::Closed { conn } => {
                self.outs.remove(&conn);
                self.call(move |relay, now| relay.on_closed(conn, now)).await;
            }
            Cmd::Image { job, reply } => self.image(job, reply).await,
            Cmd::Shutdown { done } => {
                let _ = done.send(());
            }
        }
    }

    async fn image(&mut self, job: ImageJob, reply: oneshot::Sender<ImageResponse>) {
        let Some(mut relay) = self.relay.take() else {
            let _ = reply.send(ImageResponse {
                status: ImageStatus::Unavailable,
                retry_after_ms: None,
                body: drafftink_relay::ImageBody::Empty,
            });
            return;
        };
        let now = now_ms();
        let run = tokio::task::spawn_blocking(move || {
            let op = match &job.op {
                OwnedImageOp::Put { name, body } => ImageOp::Put { name: *name, body },
                OwnedImageOp::Get { name } => ImageOp::Get { name: *name },
                OwnedImageOp::List => ImageOp::List,
            };
            let (response, out) = relay.image_request(now, ImageRequest { proof: &job.proof, op });
            (relay, response, out)
        });
        match run.await {
            Ok((relay, response, out)) => {
                self.relay = Some(relay);
                self.apply(out);
                let _ = reply.send(response);
            }
            Err(e) => warn!("a sealed room task failed: {e}"),
        }
    }

    /// Runs one call of the core on the blocking pool and carries out its output. A core that panicked is gone: the
    /// room stops and its connections close.
    async fn call(&mut self, f: impl FnOnce(&mut SealedRelay, u64) -> Output + Send + 'static) {
        let Some(mut relay) = self.relay.take() else { return };
        let now = now_ms();
        match tokio::task::spawn_blocking(move || {
            let out = f(&mut relay, now);
            (relay, out)
        })
        .await
        {
            Ok((relay, out)) => {
                self.relay = Some(relay);
                self.apply(out);
            }
            Err(e) => warn!("a sealed room task failed: {e}"),
        }
    }

    fn apply(&mut self, out: Output) {
        self.wake_at = out.wake_at;
        self.deleted |= out.deleted;
        for effect in out.effects {
            match effect {
                Effect::Send { conn, frame } => self.deliver(conn, Outbound::Binary(frame)),
                Effect::Text { conn, text } => self.deliver(conn, Outbound::Text(text)),
                Effect::Close { conn } => {
                    if let Some(out) = self.outs.remove(&conn) {
                        let _ = out.try_send(Outbound::Close);
                    }
                }
            }
        }
    }

    /// Queues a message for a connection. A connection whose queue is full is cut: dropping its sender ends its
    /// writer, and the person rejoins from the last sequence number they hold.
    fn deliver(&mut self, conn: ConnId, message: Outbound) {
        let Some(out) = self.outs.get(&conn) else { return };
        if out.try_send(message).is_err() {
            self.outs.remove(&conn);
        }
    }
}
