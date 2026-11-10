//! The bounded queue between a room and one connection's writer.
//!
//! A room never waits for a slow reader. It tries to queue each message. When the queue is
//! full (by count or by bytes), the message is dropped and the connection is marked as lagging.
//! The writer then discards the queued relays, sends `ResyncRequired`, and the client answers
//! with `RequestSync`. Nothing is dropped silently except presence, which is allowed to drop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use drafftink_protocol::ServerMessage;
use tokio::sync::{Notify, mpsc};

/// How the queue treats a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A relayed edit. Dropped when the connection lags, which then needs a resync.
    Relay,
    /// A reply the client waits for. If it cannot be queued, the connection needs a resync.
    Control,
    /// May be lost without harm (presence, acknowledgements, pongs).
    BestEffort,
    /// The last message. The connection closes after it.
    Final,
}

/// One queued item. `msg` is `None` for a plain close.
pub struct Outbound {
    pub msg: Option<Arc<ServerMessage>>,
    pub kind: Kind,
    pub bytes: usize,
}

pub(crate) struct Shared {
    lagged: AtomicBool,
    kill: AtomicBool,
    queued_bytes: AtomicUsize,
    max_bytes: usize,
    wake: Notify,
}

/// The sending half, cloneable and cheap.
#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::Sender<Outbound>,
    shared: Arc<Shared>,
}

/// The receiving half, owned by the writer task.
pub struct OutboxReceiver {
    rx: mpsc::Receiver<Outbound>,
    shared: Arc<Shared>,
}

/// Creates a queue holding at most `messages` items and `max_bytes` bytes.
pub fn outbox(messages: usize, max_bytes: usize) -> (Outbox, OutboxReceiver) {
    let (tx, rx) = mpsc::channel(messages.max(2));
    let shared = Arc::new(Shared {
        lagged: AtomicBool::new(false),
        kill: AtomicBool::new(false),
        queued_bytes: AtomicUsize::new(0),
        max_bytes,
        wake: Notify::new(),
    });
    (Outbox { tx, shared: shared.clone() }, OutboxReceiver { rx, shared })
}

/// Rough payload size of a message, for the byte bound.
pub fn approx_len(msg: &ServerMessage) -> usize {
    64 + match msg {
        ServerMessage::Update { update, .. } => update.len(),
        ServerMessage::SyncResponse { data, server_version, .. } => data.len() + server_version.len(),
        ServerMessage::Joined { server_version, peers, .. } => server_version.len() + peers.len() * 64,
        ServerMessage::PeerAwareness { state, .. } => 64 + state.selection.iter().map(String::len).sum::<usize>(),
        ServerMessage::Error { message, .. } => message.len(),
        _ => 0,
    }
}

impl Outbox {
    /// Queues a message without waiting. Returns whether it was queued.
    pub fn send(&self, msg: Arc<ServerMessage>, kind: Kind) -> bool {
        let bytes = approx_len(&msg);
        self.push(Outbound { msg: Some(msg), kind, bytes })
    }

    /// Asks the writer to close the socket after the queued messages.
    pub fn close(&self) {
        self.push(Outbound { msg: None, kind: Kind::Final, bytes: 0 });
    }

    /// Closes the socket at once, without sending the queue.
    pub fn kill(&self) {
        self.shared.kill.store(true, Ordering::SeqCst);
        self.shared.wake.notify_one();
    }

    fn push(&self, item: Outbound) -> bool {
        let (kind, bytes) = (item.kind, item.bytes);
        let before = self.shared.queued_bytes.fetch_add(bytes, Ordering::SeqCst);
        let over_bytes = before > 0 && before.saturating_add(bytes) > self.shared.max_bytes;
        let queued = !over_bytes && self.tx.try_send(item).is_ok();
        if queued {
            return true;
        }
        self.shared.queued_bytes.fetch_sub(bytes, Ordering::SeqCst);
        match kind {
            Kind::Relay | Kind::Control => {
                self.shared.lagged.store(true, Ordering::SeqCst);
                self.shared.wake.notify_one();
            }
            Kind::Final => self.kill(),
            Kind::BestEffort => {}
        }
        false
    }

    /// Whether the writer has gone away.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// What the writer does next.
pub enum Next {
    /// Send this message.
    Send(Arc<ServerMessage>),
    /// Send this message, then close.
    SendAndClose(Arc<ServerMessage>),
    /// Close now.
    Close,
}

impl OutboxReceiver {
    /// The next thing to write. After a lag it first discards queued relays and yields the
    /// `ResyncRequired` error; it then yields the control messages that were queued.
    pub async fn next(&mut self, pending: &mut std::collections::VecDeque<Next>) -> Next {
        loop {
            if let Some(next) = pending.pop_front() {
                return next;
            }
            if self.shared.kill.load(Ordering::SeqCst) {
                return Next::Close;
            }
            if self.shared.lagged.swap(false, Ordering::SeqCst) {
                let mut keep = Vec::new();
                while let Ok(item) = self.rx.try_recv() {
                    self.shared.queued_bytes.fetch_sub(item.bytes, Ordering::SeqCst);
                    if item.kind != Kind::Relay {
                        keep.push(item);
                    }
                }
                pending.push_back(Next::Send(Arc::new(ServerMessage::error(
                    drafftink_protocol::ErrorCode::ResyncRequired,
                    "this connection fell behind and dropped updates; send RequestSync with your version",
                ))));
                pending.extend(keep.into_iter().map(Self::into_next));
                continue;
            }
            tokio::select! {
                item = self.rx.recv() => match item {
                    None => return Next::Close,
                    Some(item) => {
                        self.shared.queued_bytes.fetch_sub(item.bytes, Ordering::SeqCst);
                        pending.push_back(Self::into_next(item));
                    }
                },
                () = self.shared.wake.notified() => {}
            }
        }
    }

    fn into_next(item: Outbound) -> Next {
        match (item.msg, item.kind) {
            (Some(m), Kind::Final) => Next::SendAndClose(m),
            (Some(m), _) => Next::Send(m),
            (None, _) => Next::Close,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use drafftink_protocol::{ClientId, ErrorCode};

    use super::*;

    fn update(n: usize) -> Arc<ServerMessage> {
        Arc::new(ServerMessage::Update { from: ClientId::from_bytes([1; 16]), update: vec![0; n] })
    }

    #[tokio::test]
    async fn queued_messages_arrive_in_order() {
        let (tx, mut rx) = outbox(8, 1 << 20);
        assert!(tx.send(Arc::new(ServerMessage::Ack { seq: 1 }), Kind::Control));
        assert!(tx.send(update(3), Kind::Relay));
        let mut p = VecDeque::new();
        assert!(matches!(rx.next(&mut p).await, Next::Send(m) if matches!(*m, ServerMessage::Ack { seq: 1 })));
        assert!(matches!(rx.next(&mut p).await, Next::Send(m) if matches!(*m, ServerMessage::Update { .. })));
    }

    #[tokio::test]
    async fn overflow_drops_relays_and_demands_a_resync_but_keeps_control_messages() {
        let (tx, mut rx) = outbox(3, 1 << 20);
        assert!(tx.send(update(1), Kind::Relay));
        assert!(tx.send(Arc::new(ServerMessage::Ack { seq: 9 }), Kind::Control));
        assert!(tx.send(update(2), Kind::Relay));
        assert!(!tx.send(update(3), Kind::Relay), "the queue is full");
        let mut p = VecDeque::new();
        match rx.next(&mut p).await {
            Next::Send(m) => assert!(matches!(&*m, ServerMessage::Error { code: ErrorCode::ResyncRequired, .. })),
            _ => panic!("expected ResyncRequired first"),
        }
        match rx.next(&mut p).await {
            Next::Send(m) => assert!(matches!(&*m, ServerMessage::Ack { seq: 9 })),
            _ => panic!("the control message must survive"),
        }
        assert!(tx.send(update(4), Kind::Relay), "the queue accepts messages again");
    }

    #[tokio::test]
    async fn the_byte_bound_also_triggers_a_resync() {
        let (tx, mut rx) = outbox(100, 1000);
        assert!(tx.send(update(600), Kind::Relay));
        assert!(!tx.send(update(600), Kind::Relay));
        let mut p = VecDeque::new();
        assert!(
            matches!(rx.next(&mut p).await, Next::Send(m) if matches!(&*m, ServerMessage::Error { code: ErrorCode::ResyncRequired, .. }))
        );
        assert!(tx.send(update(600), Kind::Relay), "bytes are released after the drain");
    }

    #[tokio::test]
    async fn a_single_large_message_is_accepted_into_an_empty_queue() {
        let (tx, _rx) = outbox(4, 100);
        assert!(tx.send(update(5000), Kind::Control));
    }

    #[tokio::test]
    async fn best_effort_messages_are_lost_without_a_resync() {
        let (tx, mut rx) = outbox(2, 1 << 20);
        assert!(tx.send(update(1), Kind::Relay));
        assert!(tx.send(update(1), Kind::Relay));
        assert!(!tx.send(Arc::new(ServerMessage::Pong { nonce: 1 }), Kind::BestEffort));
        let mut p = VecDeque::new();
        assert!(matches!(rx.next(&mut p).await, Next::Send(m) if matches!(&*m, ServerMessage::Update { .. })));
    }

    #[tokio::test]
    async fn kill_closes_at_once_and_a_dropped_sender_closes_the_receiver() {
        let (tx, mut rx) = outbox(4, 1 << 20);
        assert!(tx.send(update(1), Kind::Relay));
        tx.kill();
        let mut p = VecDeque::new();
        assert!(matches!(rx.next(&mut p).await, Next::Close));
        let (tx, mut rx) = outbox(4, 1 << 20);
        drop(tx);
        assert!(matches!(rx.next(&mut p).await, Next::Close));
    }
}
