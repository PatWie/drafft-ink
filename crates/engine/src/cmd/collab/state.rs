//! The collab state a session keeps between commands: the chosen server, the running client, the local laser
//! trail and the notices waiting for the UI. It lives in the session's extension map, outside the rollback
//! transaction, and is written last by every command that changes it.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use drafftink_collab::{AuthToken, CollabSession, Problem, ServerUrl, Transport, TransportError, TransportEvent};
use drafftink_protocol::RoomId;

use crate::state::{TabId, View};

/// A message at least this big is held back for one poll, so the UI can draw a progress hint before the
/// import blocks the thread (a snapshot of 100k elements takes about 1.5 s).
pub(super) const BIG_MESSAGE_BYTES: u64 = 256 * 1024;

/// The same limit as the size from which the replicated store decodes a message in slices.
pub(super) fn stage_threshold() -> usize {
    usize::try_from(BIG_MESSAGE_BYTES).unwrap_or(usize::MAX)
}

/// Most notices kept; the oldest are dropped.
const MAX_NOTICES: usize = 32;
/// Most points of the local laser trail.
const MAX_LASER_DOTS: usize = 512;

/// Something to tell the person, once.
#[derive(Debug, Clone, PartialEq)]
pub enum CollabNotice {
    /// News such as "Ana joined".
    Info(String),
    /// A failure with a plain-words text and the button to offer (`Problem::action`).
    Problem(Problem),
}

/// The big message, if the gate holds one, and whether the owner released it.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Gate {
    /// Size of the held message.
    pub held: Option<u64>,
    /// The owner agreed to import it now.
    pub release: bool,
    /// The tab can take a big message in slices, so nothing needs to be held back: the import does not stall the
    /// thread. Set by every poll before it pumps.
    pub stage: bool,
}

/// Wraps the shell's transport and holds one big message until the session releases it. Everything else passes
/// through unchanged.
pub(super) struct GatedTransport {
    inner: Box<dyn Transport>,
    gate: Rc<Cell<Gate>>,
    stash: Option<TransportEvent>,
}

impl GatedTransport {
    pub(super) fn new(inner: Box<dyn Transport>, gate: Rc<Cell<Gate>>) -> Self {
        Self { inner, gate, stash: None }
    }
}

impl Transport for GatedTransport {
    fn open(&mut self, url: &ServerUrl) -> Result<(), TransportError> {
        self.inner.open(url)
    }

    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        self.inner.send(frame)
    }

    fn close(&mut self) {
        self.inner.close();
    }

    fn poll(&mut self) -> Option<TransportEvent> {
        let mut gate = self.gate.get();
        if let Some(event) = self.stash.take() {
            if !gate.release {
                self.stash = Some(event);
                return None;
            }
            gate = Gate::default();
            self.gate.set(gate);
            return Some(event);
        }
        let event = self.inner.poll()?;
        if let TransportEvent::Message(bytes) = &event {
            let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            if len >= BIG_MESSAGE_BYTES && !gate.stage {
                self.gate.set(Gate { held: Some(len), release: false, stage: false });
                self.stash = Some(event);
                return None;
            }
        }
        Some(event)
    }
}

/// The running client of one tab.
pub(super) struct Live {
    /// The client with its transport.
    pub session: CollabSession<GatedTransport>,
    /// The tab bound to the room.
    pub tab: TabId,
    /// The room.
    pub room: RoomId,
    /// Shared with the transport.
    pub gate: Rc<Cell<Gate>>,
    /// The tab's `TabDoc::local_writes` as last seen; a different value means the peers lack an update.
    pub seen_writes: u64,
    /// True once the first catch-up is done.
    pub synced_once: bool,
    /// The poll that announced a held message has been made; the next one releases it.
    pub announced_hold: bool,
    /// The camera as the last follow step set it. A different camera means the user moved the view.
    pub follow_view: Option<View>,
}

/// One point of the local laser.
#[derive(Debug, Clone, Copy)]
struct Dot {
    x: f64,
    y: f64,
    at_ms: u64,
    fade_ms: u64,
}

/// The local laser trail: points that expire after their own fade time.
#[derive(Debug, Default)]
pub(super) struct LaserTrail {
    dots: VecDeque<Dot>,
}

impl LaserTrail {
    pub(super) fn push(&mut self, now_ms: u64, fade_ms: u64, x: f64, y: f64) {
        self.prune(now_ms);
        if self.dots.len() >= MAX_LASER_DOTS {
            self.dots.pop_front();
        }
        self.dots.push_back(Dot { x, y, at_ms: now_ms, fade_ms });
    }

    /// Drops what has faded.
    pub(super) fn prune(&mut self, now_ms: u64) {
        self.dots.retain(|d| now_ms.saturating_sub(d.at_ms) < d.fade_ms);
    }

    /// True when no point is stored.
    pub(super) fn is_empty(&self) -> bool {
        self.dots.is_empty()
    }

    pub(super) fn dots(&self, now_ms: u64) -> impl Iterator<Item = super::view::LaserDot> + '_ {
        self.dots.iter().filter(move |d| now_ms.saturating_sub(d.at_ms) < d.fade_ms).map(move |d| super::view::LaserDot {
            x: d.x,
            y: d.y,
            age_ms: now_ms.saturating_sub(d.at_ms),
            fade_ms: d.fade_ms,
        })
    }
}

/// The session-wide collab state.
#[derive(Default)]
pub(super) struct CollabState {
    /// The server chosen with `collab.connect`; kept while rooms come and go.
    pub server: Option<ServerUrl>,
    /// The access token given with the server, used by the next join. Forgotten with the server.
    pub token: Option<AuthToken>,
    /// The running client.
    pub live: Option<Live>,
    /// This user's laser trail.
    pub laser: LaserTrail,
    /// Notices waiting for the UI.
    notices: VecDeque<CollabNotice>,
}

impl CollabState {
    pub(super) fn notify(&mut self, notice: CollabNotice) {
        if self.notices.len() >= MAX_NOTICES {
            self.notices.pop_front();
        }
        self.notices.push_back(notice);
    }

    pub(super) fn take_notices(&mut self) -> Vec<CollabNotice> {
        self.notices.drain(..).collect()
    }
}
