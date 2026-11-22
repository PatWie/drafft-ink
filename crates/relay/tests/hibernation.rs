#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A Durable Object hibernates and the host rebuilds the core over the same stores while the sockets stay open. After
//! the restore calls and the first wake the core behaves as if it had never slept.

mod common;

use common::*;
use drafftink_protocol::v2::{ClientMessage, ErrorCode, ServerMessage};
use drafftink_relay::{CompactRules, ConnId, MemoryBlobStore, MemoryRelayStore, Relay, RelayConfig};
use proptest::prelude::*;

const TTL: u64 = 48 * HOUR;

/// Saves what a host saves with each socket, drops the core and restores every connection into a new one.
fn sleep_and_wake(old: Harness, members: &[ConnId], waiting: &[(ConnId, u64)]) -> Harness {
    let saved: Vec<(ConnId, _, u64)> =
        members.iter().map(|c| (*c, old.relay.member_of(*c).unwrap())).map(|(c, (id, o))| (c, id, o)).collect();
    let heard = old.now;
    let mut woken = Harness::wake_from(old);
    for (conn, id, order) in saved {
        woken.restore_member(conn, id, order, heard);
    }
    for (conn, at) in waiting {
        woken.restore_waiting(*conn, *at);
    }
    woken.tick();
    woken
}

fn two_members() -> (Harness, ConnId, ConnId) {
    let mut h = Harness::self_host();
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    (h, a, b)
}

#[test]
fn member_of_names_a_joined_connection_and_nobody_else() {
    let mut h = Harness::self_host();
    let waiting = h.connect();
    let (a, b) = (h.open(1), h.open(2));
    let (id_a, order_a) = h.relay.member_of(a).unwrap();
    let (id_b, order_b) = h.relay.member_of(b).unwrap();
    assert_eq!((id_a, id_b), (client(1), client(2)));
    assert!(order_a < order_b, "the longest-connected person has the smallest order");
    assert_eq!(h.relay.member_of(waiting), None, "not joined");
    assert_eq!(h.relay.member_of(ConnId(999)), None, "unknown");
    h.close(a);
    assert_eq!(h.relay.member_of(a), None, "gone");
}

#[test]
fn a_restore_sends_nothing_and_only_asks_for_a_wake() {
    let (mut h, a, b) = two_members();
    h.send(a, &ClientMessage::Append { record: update(client(1), 1) });
    h.advance(2 * SECOND);
    h.take(a);
    h.take(b);
    let saved = h.relay.member_of(b).unwrap();
    let mut woken = Harness::wake_from(h);
    let out = woken.relay.restore_member(b, saved.0, saved.1, woken.now, woken.now);
    assert!(out.effects.is_empty() && out.store_ops.is_empty() && !out.deleted && !out.quota_hit);
    assert!(out.wake_at.is_some());
    let out = woken.relay.restore_waiting(ConnId(77), woken.now, woken.now);
    assert!(out.effects.is_empty() && out.store_ops.is_empty());
    assert_eq!((woken.relay.member_count(), woken.relay.connection_count()), (1, 2));
    assert_eq!(woken.store.segments_written(), 1, "the restore wrote nothing");
}

#[test]
fn after_a_restore_edits_fan_out_are_acknowledged_and_a_leave_is_announced() {
    let (mut h, a, b) = two_members();
    h.send(a, &ClientMessage::Append { record: update(client(1), 1) });
    h.advance(2 * SECOND);
    h.take(a);
    h.take(b);
    let mut h = sleep_and_wake(h, &[a, b], &[]);
    assert!(h.take(a).is_empty() && h.take(b).is_empty(), "the first wake after a restore says nothing");

    h.send(a, &ClientMessage::Append { record: update(client(1), 2) });
    let seen = records(&h.take(b));
    assert_eq!(
        seen.iter().map(|r| (r.seq, r.ctr)).collect::<Vec<_>>(),
        [(2, 2)],
        "numbering goes on after the old head"
    );
    h.advance(2 * SECOND);
    assert_eq!(acks(&h.take(a)), [(2, 2)]);

    h.close(a);
    assert!(h.take(b).iter().any(|m| matches!(m, ServerMessage::PeerLeft { client_id } if *client_id == client(1))));
}

#[test]
fn join_order_survives_so_the_longest_connected_member_is_asked_to_compact() {
    let cfg = RelayConfig {
        compact: CompactRules { records: 3, bytes: u64::MAX, join_records: 1_000, advice_timeout_ms: 30_000 },
        ..RelayConfig { retention: drafftink_relay::Retention::Never, ..demo() }
    };
    let mut h = Harness::new(cfg);
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    // The host restores the newer member first: the order comes from the saved number, not from the call order.
    let mut h = sleep_and_wake(h, &[b, a], &[]);
    for ctr in 1..=3 {
        h.send(b, &ClientMessage::Append { record: update(client(2), ctr) });
    }
    h.advance(2 * SECOND);
    assert!(h.take(a).iter().any(|m| matches!(m, ServerMessage::CompactAdvice { .. })), "a joined first");
    assert!(!h.take(b).iter().any(|m| matches!(m, ServerMessage::CompactAdvice { .. })));
    let c = h.open(3);
    assert!(h.relay.member_of(c).unwrap().1 > h.relay.member_of(b).unwrap().1, "new joins number above the restored");
}

#[test]
fn a_socket_that_had_not_joined_is_refused_at_its_original_time() {
    let (mut h, a, b) = two_members();
    let waiting = h.connect();
    let connected = h.now;
    h.advance(4 * SECOND);
    let mut h = sleep_and_wake(h, &[a, b], &[(waiting, connected)]);
    assert!(h.wake_at.is_some_and(|at| at <= connected + h.relay.config().join_timeout_ms));
    h.advance(5 * SECOND);
    assert!(!h.is_closed(waiting), "10 s from when it connected have not passed");
    h.advance(2 * SECOND);
    assert!(h.is_closed(waiting));
    assert_eq!(error_code(&h.take(waiting)), Some(ErrorCode::NotJoined));
    assert!(!h.is_closed(a) && !h.is_closed(b));
}

#[test]
fn a_restored_waiting_socket_can_still_join() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    let waiting = h.connect();
    let connected = h.now;
    let mut h = sleep_and_wake(h, &[a], &[(waiting, connected)]);
    h.send(waiting, &join_message(client(2), drafftink_protocol::v2::JoinMode::Open, 0));
    assert!(h.take(waiting).iter().any(|m| matches!(m, ServerMessage::Joined { .. })));
    assert!(h.take(a).iter().any(|m| matches!(m, ServerMessage::PeerJoined { .. })));
}

/// Sends a burst of presence from `from` and returns how many entries `to` received over the next 200 ms.
fn presence_delivered(h: &mut Harness, from: ConnId, to: ConnId, sender: u8) -> usize {
    for _ in 0..200 {
        h.send(from, &ClientMessage::Presence { record: presence(client(sender)) });
    }
    h.advance(200);
    h.take(to).iter().map(|m| if let ServerMessage::PresenceBundle(entries) = m { entries.len() } else { 0 }).sum()
}

#[test]
fn presence_after_a_restore_follows_the_same_share_as_without_a_sleep() {
    let (mut control, a, b) = two_members();
    let (mut sleeper, sa, sb) = two_members();
    sleeper.advance(30 * SECOND);
    control.advance(30 * SECOND);
    let mut sleeper = sleep_and_wake(sleeper, &[sa, sb], &[]);
    let expected = presence_delivered(&mut control, a, b, 1);
    let got = presence_delivered(&mut sleeper, sa, sb, 1);
    assert!(expected > 0);
    assert_eq!(got, expected);
    // A third person who joins gets the share for three, and the restored member's rate follows it.
    let c = sleeper.open(3);
    let joined = sleeper.take(c);
    let share = joined.iter().find_map(|m| match m {
        ServerMessage::Joined { limits, .. } => Some(limits.max_presence_per_sec),
        _ => None,
    });
    assert_eq!(share, Some(sleeper.relay.config().presence_budget / 3));
}

// ---- liveness ---------------------------------------------------------------------------------------------------

fn demo_member() -> (Harness, ConnId) {
    let mut h = Harness::new(demo());
    let a = h.open(1);
    h.take(a);
    (h, a)
}

#[test]
fn a_restored_member_that_was_never_heard_again_does_not_hold_an_expired_room() {
    let (h, a) = demo_member();
    let mut h = sleep_and_wake(h, &[a], &[]);
    h.advance(TTL + SECOND);
    assert!(h.deleted, "the member was last heard two days ago");
}

#[test]
fn a_heard_ping_keeps_the_member_live_and_the_room_expiring() {
    let (h, a) = demo_member();
    let mut h = sleep_and_wake(h, &[a], &[]);
    h.advance(TTL - SECOND);
    h.relay.heard(a, h.now);
    h.advance(2 * SECOND);
    assert!(!h.deleted);
    assert!(h.take(a).iter().any(|m| matches!(m, ServerMessage::Retention { expiring: true, .. })));
    assert_eq!(h.wake_at, Some(h.now + HOUR), "the one hour recheck");
    // Silent for the recheck: the runtime answered no ping in the last two minutes.
    h.advance(HOUR + SECOND);
    assert!(h.deleted);
}

#[test]
fn heard_never_moves_back_and_ignores_unknown_connections() {
    let (h, a) = demo_member();
    let mut h = sleep_and_wake(h, &[a], &[]);
    h.advance(TTL - SECOND);
    h.relay.heard(a, h.now);
    h.relay.heard(a, 0);
    h.relay.heard(ConnId(4242), h.now);
    assert_eq!(h.relay.connection_count(), 1, "an unknown connection is not created");
    h.advance(2 * SECOND);
    assert!(!h.deleted, "the earlier time did not replace the later one");
}

// ---- retention after a restore ----------------------------------------------------------------------------------

#[test]
fn an_expired_room_with_nobody_in_it_is_deleted_by_the_first_wake_after_the_restore() {
    let (mut h, a) = demo_member();
    h.close(a);
    h.advance(TTL - 10 * SECOND);
    let mut h = sleep_and_wake(h, &[], &[]);
    h.advance(30 * SECOND);
    assert!(h.deleted);
}

#[test]
fn a_live_restored_member_makes_the_room_expiring_and_the_last_close_starts_the_grace() {
    let (mut h, a) = demo_member();
    h.pingers.push(a);
    h.advance(TTL - SECOND);
    let mut h = sleep_and_wake(h, &[a], &[]);
    h.advance(2 * SECOND);
    assert!(h.take(a).iter().any(|m| matches!(m, ServerMessage::Retention { expiring: true, .. })));
    assert_eq!(h.wake_at, Some(h.now + HOUR));
    h.close(a);
    h.advance(59 * SECOND);
    assert!(!h.deleted, "the grace of 60 s is running");
    h.advance(2 * SECOND);
    assert!(h.deleted);
}

#[test]
fn a_rejoin_after_a_restore_inside_the_grace_keeps_the_room() {
    let (mut h, a) = demo_member();
    h.pingers.push(a);
    h.advance(TTL - SECOND);
    let mut h = sleep_and_wake(h, &[a], &[]);
    h.advance(2 * SECOND);
    h.close(a);
    h.advance(30 * SECOND);
    let b = h.join(client(1), drafftink_protocol::v2::JoinMode::Open, 0);
    assert!(h.take(b).iter().any(|m| matches!(m, ServerMessage::Joined { .. })));
    h.advance(2 * HOUR);
    assert!(!h.deleted);
}

#[test]
fn a_self_hosted_room_asks_for_no_timer_after_the_first_wake() {
    let (h, a, b) = two_members();
    let mut h = sleep_and_wake(h, &[a, b], &[]);
    assert_eq!(h.wake_at, None);
    h.advance(1_000 * HOUR);
    assert!(!h.deleted);
}

// ---- hostile numbers --------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn restoring_with_any_numbers_never_panics(
        members in proptest::collection::vec((any::<u64>(), any::<u64>(), any::<u64>()), 0..8),
        waiting in proptest::collection::vec(any::<u64>(), 0..4),
        now in prop_oneof![Just(0u64), Just(u64::MAX), any::<u64>()],
        later in any::<u64>(),
    ) {
        let mut relay = Relay::new(room_id(), RelayConfig::demo(), MemoryRelayStore::new(), MemoryBlobStore::new());
        for (i, (order, heard, _)) in members.iter().enumerate() {
            let id = u8::try_from(i).unwrap();
            let _ = relay.restore_member(ConnId(u64::from(id)), client(id), *order, *heard, now);
        }
        for (i, at) in waiting.iter().enumerate() {
            let _ = relay.restore_waiting(ConnId(100 + i as u64), *at, now);
        }
        for (i, (_, _, at)) in members.iter().enumerate() {
            relay.heard(ConnId(i as u64), *at);
        }
        let _ = relay.on_wake(now);
        let _ = relay.on_wake(later);
        let joined = relay.on_frame(ConnId(500), now, &[]);
        let _ = (joined, relay.connect(ConnId(501), later), relay.on_closed(ConnId(0), later));
        let _ = relay.member_of(ConnId(1));
    }
}
