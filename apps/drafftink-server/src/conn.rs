//! One WebSocket connection: decode frames, enforce limits, talk to the room.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use drafftink_protocol::{ClientId, ClientMessage, ErrorCode, ServerMessage, negotiate_version};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::OwnedSemaphorePermit;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::http::AppState;
use crate::limits::TokenBucket;
use crate::outbox::{Kind, Next, OutboxReceiver, outbox};
use crate::room::{PeerSetup, RoomCmd, RoomHandle};
use crate::store::{StoreError, check_room};

/// Longest a single socket write may take before the peer counts as dead.
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);
/// Control messages (`RequestSync`, `Ping`, `Pong`) per second.
const CONTROL_PER_SEC: u32 = 10;
/// Rate-limit breaches further apart than this start counting again from zero.
const BREACH_MEMORY: Duration = Duration::from_secs(10);

fn encode(msg: &ServerMessage, json: bool) -> Option<Message> {
    if json {
        match msg.to_json() {
            Ok(t) => Some(Message::Text(t.into())),
            Err(e) => {
                warn!("cannot encode a message: {e}");
                None
            }
        }
    } else {
        match msg.encode() {
            Ok(b) => Some(Message::Binary(b.into())),
            Err(e) => {
                warn!("cannot encode a message: {e}");
                None
            }
        }
    }
}

async fn writer(mut sink: SplitSink<WebSocket, Message>, mut rx: OutboxReceiver, json: Arc<AtomicBool>) {
    let mut pending = VecDeque::new();
    loop {
        let (msg, close) = match rx.next(&mut pending).await {
            Next::Send(m) => (Some(m), false),
            Next::SendAndClose(m) => (Some(m), true),
            Next::Close => (None, true),
        };
        if let Some(m) = msg
            && let Some(frame) = encode(&m, json.load(Ordering::Relaxed))
            && !matches!(tokio::time::timeout(WRITE_TIMEOUT, sink.send(frame)).await, Ok(Ok(())))
        {
            break;
        }
        if close {
            let _ = tokio::time::timeout(Duration::from_secs(2), sink.send(Message::Close(None))).await;
            break;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), sink.close()).await;
}

enum Flow {
    Continue,
    Close,
}

struct Conn {
    app: Arc<AppState>,
    id: u64,
    out: crate::outbox::Outbox,
    json: Arc<AtomicBool>,
    joined: Option<RoomHandle>,
    updates: TokenBucket,
    awareness: TokenBucket,
    control: TokenBucket,
    breaches: u32,
    last_breach: Option<Instant>,
    last_ping_nonce: u64,
}

/// Runs a connection until it ends.
pub async fn serve(socket: WebSocket, app: Arc<AppState>, _permit: OwnedSemaphorePermit) {
    let (sink, mut stream) = socket.split();
    let (out, rx) = outbox(app.config.limits.outbox_messages, app.config.limits.outbox_bytes);
    let json = Arc::new(AtomicBool::new(false));
    let mut writer_task: JoinHandle<()> = tokio::spawn(writer(sink, rx, json.clone()));
    let now = Instant::now();
    let l = &app.config.limits;
    let mut conn = Conn {
        id: app.new_conn_id(),
        out: out.clone(),
        json,
        joined: None,
        updates: TokenBucket::new(u32::from(l.max_updates_per_sec), u32::from(l.max_updates_per_sec) * 2, now),
        awareness: TokenBucket::new(u32::from(l.max_awareness_per_sec), u32::from(l.max_awareness_per_sec) * 2, now),
        control: TokenBucket::new(CONTROL_PER_SEC, CONTROL_PER_SEC * 2, now),
        breaches: 0,
        last_breach: None,
        last_ping_nonce: 0,
        app: app.clone(),
    };
    let timing = &app.config.timing;
    let join_deadline = now + timing.join_timeout;
    let mut last_rx = now;
    let mut ping = tokio::time::interval_at(now + timing.ping_interval, timing.ping_interval);
    let mut writer_alive = true;

    loop {
        let idle_deadline = last_rx + timing.idle_timeout;
        tokio::select! {
            biased;
            _ = &mut writer_task => {
                writer_alive = false;
                break;
            }
            frame = stream.next() => {
                last_rx = Instant::now();
                match frame {
                    None => break,
                    Some(Err(e)) => {
                        let text = e.to_string();
                        if text.contains("too long") || text.contains("Space limit") {
                            conn.fail(ErrorCode::TooLarge, "the message is larger than the server accepts");
                        } else {
                            debug!("connection error: {text}");
                        }
                        break;
                    }
                    Some(Ok(message)) => {
                        if matches!(conn.on_message(message).await, Flow::Close) {
                            break;
                        }
                    }
                }
            }
            _ = ping.tick(), if conn.joined.is_some() => {
                conn.last_ping_nonce = conn.last_ping_nonce.wrapping_add(1);
                out.send(Arc::new(ServerMessage::Ping { nonce: conn.last_ping_nonce }), Kind::BestEffort);
            }
            () = tokio::time::sleep_until(join_deadline), if conn.joined.is_none() => {
                conn.fail(ErrorCode::NotJoined, "no Join arrived in time");
                break;
            }
            () = tokio::time::sleep_until(idle_deadline) => {
                debug!("closing an idle connection");
                break;
            }
        }
    }

    if let Some(room) = conn.joined.take() {
        let leave = RoomCmd::Leave { conn: conn.id };
        let _ = tokio::time::timeout(Duration::from_secs(5), room.tx.send(leave)).await;
    }
    if writer_alive {
        out.close();
        if tokio::time::timeout(Duration::from_secs(3), &mut writer_task).await.is_err() {
            writer_task.abort();
        }
    }
}

impl Conn {
    /// Sends a final error. The writer closes the socket after it.
    fn fail(&self, code: ErrorCode, message: impl Into<String>) {
        self.out.send(Arc::new(ServerMessage::error(code, message)), Kind::Final);
    }

    fn fail_msg(&self, msg: ServerMessage) {
        self.out.send(Arc::new(msg), Kind::Final);
    }

    async fn on_message(&mut self, message: Message) -> Flow {
        let decoded = match message {
            Message::Binary(bytes) => ClientMessage::decode(&bytes),
            Message::Text(text) => {
                if !self.app.config.debug_json {
                    self.fail(ErrorCode::Malformed, "text frames are not accepted; send binary frames");
                    return Flow::Close;
                }
                self.json.store(true, Ordering::Relaxed);
                ClientMessage::from_json(&text)
            }
            // The WebSocket layer answers pings itself; any frame counts as activity.
            Message::Ping(_) | Message::Pong(_) => return Flow::Continue,
            Message::Close(_) => return Flow::Close,
        };
        match decoded {
            Ok(msg) => self.on_client_message(msg).await,
            Err(e) => {
                debug!("closing a connection after an undecodable frame: {e}");
                self.fail_msg(ServerMessage::error_for_decode(&e));
                Flow::Close
            }
        }
    }

    /// Counts a rate-limit breach; true when the connection has had enough.
    fn breach(&mut self, now: Instant) -> bool {
        if self.last_breach.is_some_and(|t| now.saturating_duration_since(t) > BREACH_MEMORY) {
            self.breaches = 0;
        }
        self.last_breach = Some(now);
        self.breaches += 1;
        self.breaches >= self.app.config.limits.max_breaches
    }

    /// Takes a token from `which`; on refusal tells the client (when `tell`) and says whether
    /// to close.
    fn limit(&mut self, which: Bucket, tell: bool) -> Result<(), Flow> {
        let now = Instant::now();
        let bucket = match which {
            Bucket::Update => &mut self.updates,
            Bucket::Awareness => &mut self.awareness,
            Bucket::Control => &mut self.control,
        };
        let Err(wait) = bucket.take(now) else { return Ok(()) };
        if self.breach(now) {
            self.fail(ErrorCode::RateLimited, "too many messages; the connection is closed");
            return Err(Flow::Close);
        }
        if tell {
            let ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX).max(1);
            let mut msg = ServerMessage::error(ErrorCode::RateLimited, "slow down; this message was dropped");
            if let ServerMessage::Error { retry_after_ms, .. } = &mut msg {
                *retry_after_ms = Some(ms);
            }
            self.out.send(Arc::new(msg), Kind::Control);
        }
        Err(Flow::Continue)
    }

    async fn send_to_room(&mut self, cmd: RoomCmd) -> Flow {
        let Some(room) = &self.joined else { return Flow::Close };
        if room.tx.send(cmd).await.is_err() {
            self.fail_msg(ServerMessage::Error {
                code: ErrorCode::Unavailable,
                message: "the room is not available; reconnect".into(),
                retry_after_ms: Some(1000),
            });
            return Flow::Close;
        }
        Flow::Continue
    }

    async fn on_client_message(&mut self, msg: ClientMessage) -> Flow {
        if self.joined.is_none() {
            return match msg {
                ClientMessage::Join { .. } => self.on_join(msg).await,
                _ => {
                    self.fail(ErrorCode::NotJoined, "the first message must be Join");
                    Flow::Close
                }
            };
        }
        match msg {
            ClientMessage::Join { .. } => {
                self.fail(ErrorCode::AlreadyJoined, "this connection already joined a room");
                Flow::Close
            }
            ClientMessage::Update { seq, update } => {
                if update.len() > self.app.config.limits.max_update_bytes {
                    self.fail(ErrorCode::TooLarge, "the update is larger than this server accepts");
                    return Flow::Close;
                }
                if let Err(flow) = self.limit(Bucket::Update, true) {
                    return flow;
                }
                self.send_to_room(RoomCmd::Update { conn: self.id, seq, bytes: update }).await
            }
            ClientMessage::RequestSync { version } => {
                if let Err(flow) = self.limit(Bucket::Control, true) {
                    return flow;
                }
                self.send_to_room(RoomCmd::RequestSync { conn: self.id, version }).await
            }
            ClientMessage::Awareness(state) => {
                // Presence may be dropped: over the rate it is discarded without an error.
                if let Err(flow) = self.limit(Bucket::Awareness, false) {
                    return flow;
                }
                self.send_to_room(RoomCmd::Awareness { conn: self.id, state }).await
            }
            ClientMessage::Ping { nonce } => {
                if let Err(flow) = self.limit(Bucket::Control, true) {
                    return flow;
                }
                self.out.send(Arc::new(ServerMessage::Pong { nonce }), Kind::BestEffort);
                Flow::Continue
            }
            ClientMessage::Pong { .. } => Flow::Continue,
            ClientMessage::Leave => Flow::Close,
        }
    }

    async fn on_join(&mut self, msg: ClientMessage) -> Flow {
        let ClientMessage::Join { min_version, max_version, room, client_id, auth_token, version, user } = msg else {
            return Flow::Close;
        };
        let Some(protocol_version) = negotiate_version(min_version, max_version) else {
            self.fail(ErrorCode::UnsupportedVersion, "no protocol version in common");
            return Flow::Close;
        };
        if !self.app.auth.check(auth_token.as_deref()) {
            warn!("refused a join: missing or wrong token");
            self.fail(ErrorCode::Unauthorized, "this server requires a valid token");
            return Flow::Close;
        }
        if let Err(StoreError::InvalidRoom(why)) = check_room(&room) {
            self.fail(ErrorCode::Malformed, why);
            return Flow::Close;
        }
        let setup = PeerSetup {
            conn: self.id,
            client_id: ClientId::from_bytes(*client_id.as_bytes()),
            user,
            version,
            protocol_version,
            outbox: self.out.clone(),
        };
        match self.app.manager.join(&room, setup).await {
            Ok(handle) => {
                self.joined = Some(handle);
                Flow::Continue
            }
            Err(msg) => {
                self.fail_msg(*msg);
                Flow::Close
            }
        }
    }
}

enum Bucket {
    Update,
    Awareness,
    Control,
}
