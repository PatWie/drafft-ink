//! Driving the client from the session: [`Session::collab_poll`], the read-only [`Session::collab_view`] and
//! the notices.
//!
//! A shell calls `collab_poll` when the transport wakes it, when the time it asked for arrives, and after it
//! ran commands. It is cheap when nothing is shared. Commands never wait for the network: they change the
//! client's inputs, and this function moves the bytes.

use std::collections::VecDeque;

use drafftink_collab::{CollabEvent, ConnState, FollowEnd, Problem, ProblemAction, RemoteKind, TrimmedChoice};
use drafftink_geom::kurbo::Point;
use drafftink_protocol::{MAX_SELECTION_IDS, Viewport};

use super::state::{CollabNotice, CollabState, Gate, Live};
use super::view::{CollabView, ConnectionState, Loading, PeerView};
use crate::session::Session;
use crate::spec::Redraw;
use crate::state::clamp_zoom;

/// How long after a held big message the next poll is asked for, so one frame can show the progress hint.
const HOLD_FRAME_MS: u64 = 16;
/// The frame interval while the local laser trail fades.
const LASER_FRAME_MS: u64 = 16;

/// What a poll found, for the shell's loop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollabPoll {
    /// Something visible changed: draw one frame. The session has also raised its overlay dirty reason.
    pub redraw: bool,
    /// Call `collab_poll` again after this many milliseconds if nothing else wakes the shell first. `None`:
    /// nothing is scheduled, so set no timer.
    pub wake_in_ms: Option<u64>,
}

impl CollabPoll {
    fn merge_wake(&mut self, ms: u64) {
        self.wake_in_ms = Some(self.wake_in_ms.map_or(ms, |w| w.min(ms)));
    }
}

impl Session {
    /// Moves bytes between the client and the transport, hands local commits to the client, applies what the
    /// room sent, and publishes selection and viewport. Never blocks on the network; applying a big update
    /// takes as long as the import (see [`Loading::Receiving`]).
    pub fn collab_poll(&mut self) -> CollabPoll {
        let now = self.ports.clock.mono_ms();
        let mut out = CollabPoll::default();
        if let Some(mut live) = self.ext.with::<CollabState, _>(|c| c.live.take()) {
            // A poll that finds its tab gone drops the client; closing a room tab asks to leave first.
            if let Some(step) = poll_live(self, &mut live, now) {
                out = step;
                self.ext.with::<CollabState, _>(|c| c.live = Some(live));
            }
        }
        let (was_visible, still_visible) = self.ext.with::<CollabState, _>(|c| {
            let was = !c.laser.is_empty();
            c.laser.prune(now);
            (was, !c.laser.is_empty())
        });
        out.redraw |= was_visible;
        if still_visible {
            out.merge_wake(LASER_FRAME_MS);
        }
        if out.redraw {
            self.mark_dirty(Redraw::Overlay);
        }
        out
    }

    /// The connection, the room, the people and their pointers, for the UI and the overlay.
    pub fn collab_view(&self) -> CollabView {
        let now = self.ports.clock.mono_ms();
        let Some(state) = self.ext.get::<CollabState>() else { return CollabView::disconnected() };
        let mut view = match (&state.live, &state.server) {
            (Some(live), _) => live_view(live, now),
            (None, Some(server)) => CollabView {
                state: ConnectionState::Ready,
                url: Some(server.to_string()),
                text: "Server chosen. Join a room with `collab.join_room`.".to_owned(),
                ..CollabView::disconnected()
            },
            (None, None) => CollabView::disconnected(),
        };
        view.laser = state.laser.dots(now).collect();
        view
    }

    /// Takes the notices (joins, leaves, problems) the UI has not shown yet.
    pub fn take_collab_notices(&mut self) -> Vec<CollabNotice> {
        self.ext.with::<CollabState, _>(CollabState::take_notices)
    }

    pub(super) fn collab_notify(&mut self, notice: CollabNotice) {
        self.ext.with::<CollabState, _>(|c| c.notify(notice));
    }
}

fn live_view(live: &Live, now_ms: u64) -> CollabView {
    let client = live.session.client();
    let status = client.status();
    let state = ConnectionState::of(status.state);
    let following = client.following();
    let held = live.gate.get().held;
    let loading = match (held, live.synced_once, status.state) {
        (Some(bytes), _, _) => Some(Loading::Receiving { bytes }),
        (None, false, ConnState::Idle | ConnState::Connecting | ConnState::Reconnecting) => Some(Loading::Connecting),
        (None, false, ConnState::Syncing | ConnState::Online) => Some(Loading::Syncing),
        _ => None,
    };
    CollabView {
        state,
        url: Some(client.config().url.to_string()),
        room: Some(live.room.to_string()),
        tab: Some(live.tab.to_string()),
        text: status.text.clone(),
        error: (state == ConnectionState::Error).then(|| status.text.clone()),
        unsynced: status.unsynced,
        loading,
        peers: client.peers().iter().map(|p| PeerView::of(p, now_ms, following == Some(p.id))).collect(),
        ..CollabView::disconnected()
    }
}

/// One poll of a running client. `None` when its tab no longer exists.
fn poll_live(s: &mut Session, live: &mut Live, now: u64) -> Option<CollabPoll> {
    publish_local(s, live, now)?;
    if hold_big_message(live) {
        return Some(CollabPoll { redraw: true, wake_in_ms: Some(HOLD_FRAME_MS) });
    }
    let pump = {
        let tab = s.tab_by_id_mut(live.tab)?;
        live.session.pump(now, tab.doc.replicated_mut())
    };
    let mut events: VecDeque<CollabEvent> = pump.events.into();
    while let Some(event) = events.pop_front() {
        handle(s, live, now, event, &mut events)?;
    }
    if live.session.client().status().state == ConnState::Online {
        live.synced_once = true;
    }
    let gate = live.gate.get();
    if gate.held.is_some() && !gate.release {
        // The pump just met a big message. Ask for a frame now; the next poll imports it.
        live.announced_hold = true;
        return Some(CollabPoll { redraw: true, wake_in_ms: Some(HOLD_FRAME_MS) });
    }
    let wake = live.session.client().next_wake_ms().map(|w| w.saturating_sub(now));
    Some(CollabPoll { redraw: pump.redraw, wake_in_ms: wake })
}

/// Tells the client about local commits and publishes selection and viewport; ends following when the user
/// moved the view.
fn publish_local(s: &mut Session, live: &mut Live, now: u64) -> Option<()> {
    let tab = s.tab_by_id(live.tab)?;
    let writes = tab.doc.local_writes();
    let view = tab.view;
    let (x, y, width, height) = view.visible();
    let selection: Vec<String> =
        tab.observed.selection.iter().take(MAX_SELECTION_IDS).map(|id| id.as_str().to_owned()).collect();
    let client = live.session.client_mut();
    if writes != live.seen_writes {
        live.seen_writes = writes;
        client.local_changed(now);
    }
    client.set_selection(selection);
    client.set_viewport(Some(Viewport { x, y, width, height }));
    if client.following().is_some() && live.follow_view.is_some_and(|v| v != view) {
        client.stop_following();
        live.follow_view = None;
        s.collab_notify(CollabNotice::Info("Stopped following because you moved the view.".to_owned()));
    }
    Some(())
}

/// True when the poll should stop: a big message waits and the UI gets one poll to show progress first.
fn hold_big_message(live: &mut Live) -> bool {
    let gate = live.gate.get();
    if gate.held.is_none() {
        live.announced_hold = false;
        return false;
    }
    if gate.release {
        return false;
    }
    if live.announced_hold {
        live.announced_hold = false;
        live.gate.set(Gate { release: true, ..gate });
        false
    } else {
        live.announced_hold = true;
        true
    }
}

/// Reacts to one client event. `None` when the tab is gone.
fn handle(s: &mut Session, live: &mut Live, now: u64, event: CollabEvent, rest: &mut VecDeque<CollabEvent>) -> Option<()> {
    match event {
        CollabEvent::Remote(batch) => {
            if let Err(refusal) = s.mirror_remote(live.tab, &batch.changes) {
                s.collab_notify(CollabNotice::Problem(Problem {
                    code: None,
                    text: refusal.message,
                    action: ProblemAction::Reload,
                    fatal: false,
                }));
            }
            if batch.kind == RemoteKind::Replaced {
                s.tab_by_id_mut(live.tab)?.doc.clear_history();
            }
        }
        CollabEvent::Problem(p) => s.collab_notify(CollabNotice::Problem(p)),
        CollabEvent::PeerJoined { name, .. } => s.collab_notify(CollabNotice::Info(format!("{name} joined."))),
        CollabEvent::PeerLeft { name, .. } => s.collab_notify(CollabNotice::Info(format!("{name} left."))),
        CollabEvent::FollowViewport(v) => follow_viewport(s, live, v)?,
        CollabEvent::FollowEnded(why) => {
            live.follow_view = None;
            let text = match why {
                FollowEnd::PeerLeft => "The person you followed left the room.",
                FollowEnd::Disconnected => "Stopped following because the connection was lost.",
            };
            s.collab_notify(CollabNotice::Info(text.to_owned()));
        }
        CollabEvent::NeedsDecision { message } => {
            // The user chose no replacement at join time (`on_local` other than `discard`), so keep this copy.
            let tab = s.tab_by_id_mut(live.tab)?;
            live.session.client_mut().resolve_trimmed(now, tab.doc.replicated_mut(), TrimmedChoice::KeepMine);
            let more = live.session.pump(now, tab.doc.replicated_mut());
            rest.extend(more.events);
            s.collab_notify(CollabNotice::Problem(Problem {
                code: None,
                text: format!("{message} This copy was kept and is no longer shared. Join again with on_local=\"discard\" to take the room's content."),
                action: ProblemAction::Decide,
                fatal: true,
            }));
        }
        CollabEvent::Status(_) | CollabEvent::Presence => {}
    }
    Some(())
}

/// Moves the camera of the room's tab so the followed person's rectangle fills it.
fn follow_viewport(s: &mut Session, live: &mut Live, v: Viewport) -> Option<()> {
    if v.width <= 0.0 || v.height <= 0.0 {
        return Some(());
    }
    let tab = s.tab_by_id_mut(live.tab)?;
    let screen = tab.view.viewport;
    tab.view.zoom = clamp_zoom((screen.w / v.width).min(screen.h / v.height));
    tab.view.scroll = Point::new(v.x + v.width / 2.0, v.y + v.height / 2.0);
    live.follow_view = Some(tab.view);
    s.mark_dirty(Redraw::Camera);
    Some(())
}
