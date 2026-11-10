//! Presence: what this client shows to others and what it knows about them. Ephemeral: none of it enters the
//! document, none of it is stored, and all of it can be dropped without harm (docs/protocol.md section 4).
//!
//! Everything here takes the time as an argument.

use std::collections::{BTreeMap, VecDeque};

use drafftink_protocol::{
    AWARENESS_HEARTBEAT_MS, AWARENESS_MIN_INTERVAL_MS, AWARENESS_TIMEOUT_MS, AwarenessState, ClientId,
    MAX_ELEMENT_ID_BYTES, MAX_SELECTION_IDS, PeerInfo, Point, UserInfo, Viewport,
};

/// How long a laser or cursor trail point stays visible, in milliseconds.
pub const TRAIL_TTL_MS: u64 = 1500;
/// Most points a trail keeps.
pub const TRAIL_MAX_POINTS: usize = 64;

/// A short history of pointer positions, newest last. Used for the laser pointer: the trail fades because old
/// points expire, not because anything animates.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trail {
    points: VecDeque<(u64, Point)>,
}

impl Trail {
    /// Adds a point seen at `now_ms`.
    pub fn push(&mut self, now_ms: u64, p: Point) {
        self.prune(now_ms);
        if self.points.back().is_some_and(|(_, last)| *last == p) {
            return;
        }
        if self.points.len() >= TRAIL_MAX_POINTS {
            self.points.pop_front();
        }
        self.points.push_back((now_ms, p));
    }

    fn prune(&mut self, now_ms: u64) {
        while self.points.front().is_some_and(|(t, _)| now_ms.saturating_sub(*t) > TRAIL_TTL_MS) {
            self.points.pop_front();
        }
    }

    /// The points that have not expired at `now_ms`, oldest first, with their age in milliseconds.
    pub fn points(&self, now_ms: u64) -> Vec<(u64, Point)> {
        self.points
            .iter()
            .filter(|(t, _)| now_ms.saturating_sub(*t) <= TRAIL_TTL_MS)
            .map(|(t, p)| (now_ms.saturating_sub(*t), *p))
            .collect()
    }

    /// When the newest point expires, or `None` for an empty trail. The caller wants a frame then.
    pub fn expires_at(&self) -> Option<u64> {
        self.points.back().map(|(t, _)| t.saturating_add(TRAIL_TTL_MS).saturating_add(1))
    }

    /// True when no point is stored (expired points that were not pruned yet count).
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Forgets every point.
    pub fn clear(&mut self) {
        self.points.clear();
    }
}

fn finite(p: Point) -> Option<Point> {
    (p.x.is_finite() && p.y.is_finite()).then_some(p)
}

fn sane_viewport(v: Viewport) -> Option<Viewport> {
    let ok = v.x.is_finite() && v.y.is_finite() && v.width.is_finite() && v.height.is_finite();
    (ok && v.width >= 0.0 && v.height >= 0.0).then_some(v)
}

/// This client's own presence, with the rules for when to send it.
///
/// Sending rules: at most one message per [`AWARENESS_MIN_INTERVAL_MS`] (or the server's own rate, when slower), only
/// when something changed, a full resend every [`AWARENESS_HEARTBEAT_MS`], and nothing at all while nobody else is
/// in the room.
#[derive(Debug, Clone, Default)]
pub struct LocalPresence {
    state: AwarenessState,
    laser: bool,
    laser_trail: Trail,
    dirty: bool,
    last_sent_ms: Option<u64>,
}

impl LocalPresence {
    /// A presence with a display identity and nothing else.
    pub fn new(user: Option<UserInfo>) -> Self {
        LocalPresence { state: AwarenessState { user, ..AwarenessState::default() }, dirty: true, ..Self::default() }
    }

    /// The current state, as sent.
    pub fn state(&self) -> &AwarenessState {
        &self.state
    }

    /// Changes name and colour. Values the protocol would refuse are ignored.
    pub fn set_user(&mut self, user: Option<UserInfo>) {
        if user.as_ref().is_some_and(|u| u.validate().is_err()) {
            return;
        }
        if self.state.user != user {
            self.state.user = user;
            self.dirty = true;
        }
    }

    /// Sets the pointer position in world units; `None` hides it. A non-finite position hides it.
    pub fn set_cursor(&mut self, now_ms: u64, cursor: Option<Point>) {
        let cursor = cursor.and_then(finite);
        if self.laser
            && let Some(p) = cursor
        {
            self.laser_trail.push(now_ms, p);
        }
        if self.state.cursor != cursor {
            self.state.cursor = cursor;
            self.dirty = true;
        }
    }

    /// Sets the selected element ids. Empty ids and ids over the protocol limit are dropped, and the list is cut at
    /// the limit.
    pub fn set_selection(&mut self, ids: impl IntoIterator<Item = String>) {
        let ids: Vec<String> = ids
            .into_iter()
            .filter(|id| !id.is_empty() && id.len() <= MAX_ELEMENT_ID_BYTES)
            .take(MAX_SELECTION_IDS)
            .collect();
        if self.state.selection != ids {
            self.state.selection = ids;
            self.dirty = true;
        }
    }

    /// Sets the visible world rectangle. A rectangle with non-finite or negative numbers is ignored.
    pub fn set_viewport(&mut self, viewport: Option<Viewport>) {
        let viewport = match viewport {
            Some(v) => match sane_viewport(v) {
                Some(v) => Some(v),
                None => return,
            },
            None => None,
        };
        if self.state.viewport != viewport {
            self.state.viewport = viewport;
            self.dirty = true;
        }
    }

    /// Turns the laser pointer on or off. This is local: the wire carries the cursor, and a receiver draws the
    /// trail of any peer's cursor (protocol version 1 has no laser flag).
    pub fn set_laser(&mut self, on: bool) {
        self.laser = on;
        if !on {
            self.laser_trail.clear();
        }
    }

    /// True while the laser pointer is on.
    pub fn laser(&self) -> bool {
        self.laser
    }

    /// The local laser trail.
    pub fn laser_trail(&self) -> &Trail {
        &self.laser_trail
    }

    /// Makes the next [`due`](Self::due) send now (a peer just joined and has not seen this state).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Forgets what was sent: the next `due` sends the full state at once (a new connection, or a new listener).
    pub fn reset_sent(&mut self) {
        self.last_sent_ms = None;
        self.dirty = true;
    }

    /// The minimum gap between two sends: the protocol's, or the server's when it is slower.
    pub fn min_interval_ms(max_per_sec: u16) -> u64 {
        let server = 1000u64.div_ceil(u64::from(max_per_sec.max(1)));
        server.max(u64::from(AWARENESS_MIN_INTERVAL_MS))
    }

    /// The state to send now, if any. `listening` is false while nobody else is in the room.
    pub fn due(&self, now_ms: u64, min_interval_ms: u64, listening: bool) -> Option<&AwarenessState> {
        if !listening {
            return None;
        }
        match self.next_due(now_ms, min_interval_ms, listening) {
            Some(at) if at <= now_ms => Some(&self.state),
            _ => None,
        }
    }

    /// When the next send is due, if one is. `Some(t)` with `t` in the past means now.
    pub fn next_due(&self, _now_ms: u64, min_interval_ms: u64, listening: bool) -> Option<u64> {
        if !listening {
            return None;
        }
        let Some(last) = self.last_sent_ms else { return Some(0) };
        let heartbeat = last.saturating_add(u64::from(AWARENESS_HEARTBEAT_MS));
        if self.dirty { Some(last.saturating_add(min_interval_ms).min(heartbeat)) } else { Some(heartbeat) }
    }

    /// Records that the state went out at `now_ms`.
    pub fn mark_sent(&mut self, now_ms: u64) {
        self.dirty = false;
        self.last_sent_ms = Some(now_ms);
    }
}

/// What this client knows about one other participant.
#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    /// The peer's id.
    pub id: ClientId,
    /// Name and colour.
    pub user: Option<UserInfo>,
    /// Pointer position in world units.
    pub cursor: Option<Point>,
    /// Selected element ids.
    pub selection: Vec<String>,
    /// The visible world rectangle.
    pub viewport: Option<Viewport>,
    /// When the last awareness arrived. `None` once it timed out or before the first one.
    pub last_awareness_ms: Option<u64>,
    trail: Trail,
}

impl Peer {
    fn new(id: ClientId, user: Option<UserInfo>) -> Self {
        Peer {
            id,
            user,
            cursor: None,
            selection: Vec::new(),
            viewport: None,
            last_awareness_ms: None,
            trail: Trail::default(),
        }
    }

    /// The recent path of the peer's cursor.
    pub fn trail(&self) -> &Trail {
        &self.trail
    }

    /// The display name, or a short form of the id when the peer gave none.
    pub fn display_name(&self) -> String {
        match &self.user {
            Some(u) if !u.name.trim().is_empty() => u.name.clone(),
            _ => self.id.to_string().chars().take(6).collect(),
        }
    }
}

/// The other participants of the room.
#[derive(Debug, Clone, Default)]
pub struct PeerTable {
    peers: BTreeMap<ClientId, Peer>,
}

impl PeerTable {
    /// Replaces the table with the peers listed in `Joined` (the entry for `me` is skipped).
    pub fn replace(&mut self, me: ClientId, list: &[PeerInfo]) {
        self.peers.clear();
        for info in list.iter().filter(|p| p.client_id != me) {
            self.peers.insert(info.client_id, Peer::new(info.client_id, info.user.clone()));
        }
    }

    /// Adds a peer. Returns false when it is this client or already known with the same identity.
    pub fn join(&mut self, me: ClientId, info: &PeerInfo) -> bool {
        if info.client_id == me {
            return false;
        }
        match self.peers.get_mut(&info.client_id) {
            Some(p) => {
                let changed = info.user.is_some() && p.user != info.user;
                if changed {
                    p.user = info.user.clone();
                }
                changed
            }
            None => {
                self.peers.insert(info.client_id, Peer::new(info.client_id, info.user.clone()));
                true
            }
        }
    }

    /// Removes a peer.
    pub fn leave(&mut self, id: ClientId) -> Option<Peer> {
        self.peers.remove(&id)
    }

    /// Applies an awareness message (a full replacement of the peer's presence). Returns true when something the
    /// UI shows changed.
    pub fn awareness(&mut self, me: ClientId, now_ms: u64, from: ClientId, state: AwarenessState) -> bool {
        if from == me {
            return false;
        }
        let peer = self.peers.entry(from).or_insert_with(|| Peer::new(from, None));
        let mut changed = peer.last_awareness_ms.is_none();
        peer.last_awareness_ms = Some(now_ms);
        if let Some(user) = state.user
            && peer.user.as_ref() != Some(&user)
        {
            peer.user = Some(user);
            changed = true;
        }
        if peer.cursor != state.cursor {
            peer.cursor = state.cursor;
            changed = true;
        }
        if let Some(p) = state.cursor {
            peer.trail.push(now_ms, p);
        }
        if peer.selection != state.selection {
            peer.selection = state.selection;
            changed = true;
        }
        if peer.viewport != state.viewport {
            peer.viewport = state.viewport;
            changed = true;
        }
        changed
    }

    /// Clears the presence of peers that were silent for [`AWARENESS_TIMEOUT_MS`]. The peers stay listed (the server
    /// still reports them as connected), but show no cursor. Returns the ids that changed.
    pub fn expire(&mut self, now_ms: u64) -> Vec<ClientId> {
        let mut out = Vec::new();
        for p in self.peers.values_mut() {
            let Some(last) = p.last_awareness_ms else { continue };
            if now_ms.saturating_sub(last) >= u64::from(AWARENESS_TIMEOUT_MS) {
                p.last_awareness_ms = None;
                p.cursor = None;
                p.selection.clear();
                p.viewport = None;
                p.trail.clear();
                out.push(p.id);
            }
        }
        out
    }

    /// When the next peer presence times out.
    pub fn next_expiry(&self) -> Option<u64> {
        self.peers
            .values()
            .filter_map(|p| p.last_awareness_ms)
            .min()
            .map(|t| t.saturating_add(u64::from(AWARENESS_TIMEOUT_MS)))
    }

    /// When the last trail point of any peer expires, so the caller can schedule the frame that clears it.
    pub fn next_trail_expiry(&self, now_ms: u64) -> Option<u64> {
        self.peers.values().filter_map(|p| p.trail.expires_at()).filter(|t| *t > now_ms).min()
    }

    /// Forgets everyone (the connection is gone).
    pub fn clear(&mut self) {
        self.peers.clear();
    }

    /// One peer.
    pub fn get(&self, id: &ClientId) -> Option<&Peer> {
        self.peers.get(id)
    }

    /// Every peer, ordered by id.
    pub fn iter(&self) -> impl Iterator<Item = &Peer> {
        self.peers.values()
    }

    /// The number of peers.
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// True when nobody else is here.
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> ClientId {
        ClientId::from_bytes([n; 16])
    }
    fn user(n: &str) -> UserInfo {
        UserInfo { name: n.into(), color: 0x112233 }
    }
    fn at(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn local_presence_throttles_to_one_message_per_interval() {
        let mut p = LocalPresence::new(Some(user("me")));
        let step = LocalPresence::min_interval_ms(20);
        assert_eq!(step, 50);
        assert!(p.due(0, step, true).is_some(), "the first state goes out at once");
        p.mark_sent(0);
        assert!(p.due(10, step, true).is_none(), "nothing changed");
        p.set_cursor(10, Some(at(1.0, 2.0)));
        assert!(p.due(20, step, true).is_none(), "changed, but 20 ms is inside the interval");
        assert_eq!(p.next_due(20, step, true), Some(50));
        for t in 21..50 {
            p.set_cursor(t, Some(at(t as f64, 0.0)));
        }
        assert!(p.due(49, step, true).is_none());
        assert_eq!(p.due(50, step, true).and_then(|s| s.cursor), Some(at(49.0, 0.0)), "the latest cursor goes out");
        p.mark_sent(50);
        assert!(p.due(100, step, true).is_none());
    }

    #[test]
    fn local_presence_sends_a_heartbeat_and_stays_silent_when_alone() {
        let mut p = LocalPresence::new(None);
        p.mark_sent(1000);
        let step = 50;
        assert_eq!(p.next_due(1000, step, true), Some(1000 + u64::from(AWARENESS_HEARTBEAT_MS)));
        assert!(p.due(1000 + u64::from(AWARENESS_HEARTBEAT_MS) - 1, step, true).is_none());
        assert!(p.due(1000 + u64::from(AWARENESS_HEARTBEAT_MS), step, true).is_some());
        p.set_cursor(2000, Some(at(1.0, 1.0)));
        assert_eq!(p.due(5000, step, false), None, "nobody listens");
        assert_eq!(p.next_due(5000, step, false), None);
    }

    #[test]
    fn a_slow_server_rate_widens_the_interval() {
        assert_eq!(LocalPresence::min_interval_ms(5), 200);
        assert_eq!(LocalPresence::min_interval_ms(100), 50);
        assert_eq!(LocalPresence::min_interval_ms(0), 1000);
    }

    #[test]
    fn local_setters_reject_values_the_protocol_would_refuse() {
        let mut p = LocalPresence::new(None);
        p.mark_sent(0);
        p.set_cursor(1, Some(at(f64::NAN, 0.0)));
        assert_eq!(p.state().cursor, None);
        p.set_viewport(Some(Viewport { x: 0.0, y: 0.0, width: -1.0, height: 5.0 }));
        assert_eq!(p.state().viewport, None);
        p.set_selection(["".to_owned(), "a".repeat(65), "ok".to_owned()]);
        assert_eq!(p.state().selection, vec!["ok".to_owned()]);
        p.set_selection((0..MAX_SELECTION_IDS + 10).map(|i| format!("e{i}")));
        assert_eq!(p.state().selection.len(), MAX_SELECTION_IDS);
        p.set_user(Some(UserInfo { name: "x".into(), color: 0xff00_0000 }));
        assert_eq!(p.state().user, None);
        assert!(p.state().validate().is_ok());
    }

    #[test]
    fn the_laser_trail_follows_the_cursor_only_while_on_and_fades() {
        let mut p = LocalPresence::new(None);
        p.set_cursor(0, Some(at(1.0, 1.0)));
        assert!(p.laser_trail().is_empty());
        p.set_laser(true);
        p.set_cursor(100, Some(at(2.0, 2.0)));
        p.set_cursor(200, Some(at(3.0, 3.0)));
        assert_eq!(p.laser_trail().points(200).len(), 2);
        assert_eq!(p.laser_trail().points(100 + TRAIL_TTL_MS + 1).len(), 1, "the older point faded");
        assert_eq!(p.laser_trail().expires_at(), Some(200 + TRAIL_TTL_MS + 1));
        p.set_laser(false);
        assert!(p.laser_trail().is_empty());
    }

    #[test]
    fn a_trail_is_bounded() {
        let mut t = Trail::default();
        for i in 0..1000u64 {
            t.push(i, at(i as f64, 0.0));
        }
        assert!(t.points(1000).len() <= TRAIL_MAX_POINTS);
    }

    #[test]
    fn peers_come_from_joined_and_awareness_replaces_the_whole_state() {
        let me = id(1);
        let mut t = PeerTable::default();
        t.replace(
            me,
            &[PeerInfo { client_id: me, user: None }, PeerInfo { client_id: id(2), user: Some(user("ana")) }],
        );
        assert_eq!(t.len(), 1);
        assert!(t.awareness(
            me,
            10,
            id(2),
            AwarenessState { cursor: Some(at(5.0, 5.0)), selection: vec!["a".into()], ..Default::default() }
        ));
        let p = t.get(&id(2)).unwrap();
        assert_eq!(
            (p.cursor, p.selection.len(), p.user.clone()),
            (Some(at(5.0, 5.0)), 1, Some(user("ana"))),
            "no user in the message keeps the name"
        );
        assert!(t.awareness(me, 20, id(2), AwarenessState::default()));
        let p = t.get(&id(2)).unwrap();
        assert_eq!((p.cursor, p.selection.len()), (None, 0), "a message replaces the previous state");
        assert!(!t.awareness(me, 30, id(2), AwarenessState::default()), "same state, nothing to redraw");
        assert!(!t.awareness(me, 30, me, AwarenessState::default()), "own echo ignored");
    }

    #[test]
    fn silent_peers_lose_their_cursor_but_stay_listed() {
        let me = id(1);
        let mut t = PeerTable::default();
        t.join(me, &PeerInfo { client_id: id(2), user: Some(user("ana")) });
        t.awareness(me, 1000, id(2), AwarenessState { cursor: Some(at(1.0, 1.0)), ..Default::default() });
        assert_eq!(t.next_expiry(), Some(1000 + u64::from(AWARENESS_TIMEOUT_MS)));
        assert!(t.expire(1000 + u64::from(AWARENESS_TIMEOUT_MS) - 1).is_empty());
        assert_eq!(t.expire(1000 + u64::from(AWARENESS_TIMEOUT_MS)), vec![id(2)]);
        let p = t.get(&id(2)).unwrap();
        assert_eq!(p.cursor, None);
        assert_eq!(t.next_expiry(), None);
        assert!(t.expire(u64::MAX).is_empty(), "expires once");
        assert!(t.leave(id(2)).is_some());
        assert!(t.is_empty());
    }

    #[test]
    fn display_name_falls_back_to_a_short_id() {
        let p = Peer::new(id(0xab), None);
        assert_eq!(p.display_name(), "ababab");
        let p = Peer::new(id(1), Some(user("Ana")));
        assert_eq!(p.display_name(), "Ana");
    }
}
