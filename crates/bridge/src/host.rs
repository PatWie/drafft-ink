//! The shell side of the bridge. Transport threads never touch the [`Session`]. They put a [`Job`] on a
//! bounded queue and call the waker. The shell owns a [`BridgeHost`] and calls [`BridgeHost::poll`] at the
//! top of a frame; that is the only place bridge requests run, on the shell's thread, between frames.
//!
//! ```ignore
//! let (server, mut host) = BridgeServer::start(config, Arc::new(move || ctx.request_repaint()))?;
//! // in the frame:
//! let polled = host.poll(&mut session);
//! if polled.more { /* the host already woke the shell again */ }
//! ```
//!
//! Idle stays idle: nothing here has a timer. A request wakes the shell once; a flood is run 64 per poll
//! and the host wakes the shell again until the queue is empty.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};

use drafftink_engine::Session;
use serde_json::{Value, json};

use crate::core::dispatch;
use crate::rpc::{BUSY, INTERNAL_ERROR, RpcError};

/// Requests run per poll. The rest stay queued for the next frame, so a flood cannot starve drawing.
pub const PER_POLL: usize = 64;
/// Requests that may wait. More are answered `busy` at once.
pub const QUEUE: usize = 256;

/// Wakes the shell so that it polls (for example `ctx.request_repaint()`).
pub type Waker = Arc<dyn Fn() + Send + Sync>;
/// Delivers the answer to the transport.
pub type Reply = Box<dyn FnOnce(Result<Value, RpcError>) + Send>;

/// One request waiting for the shell.
pub struct Job {
    /// The method.
    pub method: String,
    /// The parameters.
    pub params: Value,
    /// Where the answer goes.
    pub reply: Reply,
}

/// Where change notifications go. The native server implements it with a broadcast channel.
pub trait EventSink: Send + Sync {
    /// True when at least one peer wants notifications. Without one the host does no work for them.
    fn has_subscribers(&self) -> bool;
    /// Sends `{method, params}` to every subscriber.
    fn publish(&self, method: &str, params: Value);
}

/// The transport's end of the queue.
#[derive(Clone)]
pub struct JobSender {
    tx: SyncSender<Job>,
    waker: Waker,
}

impl JobSender {
    /// Queues a job and wakes the shell. A full queue or a closed shell is an error the transport sends back.
    pub fn submit(&self, job: Job) -> Result<(), (RpcError, Reply)> {
        match self.tx.try_send(job) {
            Ok(()) => {
                (self.waker)();
                Ok(())
            }
            Err(TrySendError::Full(j)) => Err((
                RpcError::new(
                    BUSY,
                    "the app has too many requests waiting. Wait for the earlier answers, then try again",
                ),
                j.reply,
            )),
            Err(TrySendError::Disconnected(j)) => {
                Err((RpcError::new(INTERNAL_ERROR, "the app is closing, so the request was not run"), j.reply))
            }
        }
    }
}

/// What one poll did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Polled {
    /// Requests run.
    pub ran: usize,
    /// More requests wait. The host has already woken the shell for them.
    pub more: bool,
}

/// The shell's end of the queue.
pub struct BridgeHost {
    rx: Receiver<Job>,
    waker: Waker,
    sink: Option<Arc<dyn EventSink>>,
    announced: (u64, u64),
}

/// Makes the two ends of the queue.
pub fn channel(waker: Waker, sink: Option<Arc<dyn EventSink>>) -> (JobSender, BridgeHost) {
    let (tx, rx) = sync_channel(QUEUE);
    (JobSender { tx, waker: waker.clone() }, BridgeHost { rx, waker, sink, announced: (0, 0) })
}

impl BridgeHost {
    /// Runs up to [`PER_POLL`] waiting requests against `session`, then announces a change to subscribers.
    /// Cheap when nothing waits: one failed `try_recv` and two integer compares.
    pub fn poll(&mut self, session: &mut Session) -> Polled {
        let mut ran = 0;
        let mut more = false;
        while ran < PER_POLL {
            match self.rx.try_recv() {
                Ok(job) => {
                    let out = dispatch(session, &job.method, &job.params);
                    (job.reply)(out);
                    ran += 1;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if ran == PER_POLL {
            // The queue may hold more. A sync_channel cannot be peeked, so ask the shell for one more poll.
            more = true;
            (self.waker)();
        }
        self.announce(session);
        Polled { ran, more }
    }

    fn announce(&mut self, session: &Session) {
        let Some(sink) = &self.sink else { return };
        let now = (session.ring().next_seq(), session.tab().doc.revision());
        if now == self.announced {
            return;
        }
        self.announced = now;
        if sink.has_subscribers() {
            sink.publish("changed", json!({ "seq": now.0, "revision": now.1 }));
        }
    }
}
