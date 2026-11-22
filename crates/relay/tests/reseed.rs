#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Reseeding an empty room is a compare-and-set: the first cache holder wins.

mod common;

use common::*;
use drafftink_protocol::v2::{ClientMessage, JoinMode, ServerMessage};

fn seed(h: &mut Harness, conn: drafftink_relay::ConnId, sender: u8) -> ServerMessage {
    h.send(conn, &ClientMessage::PutSnapshot { expected_snapshot_seq: 0, record: snapshot(client(sender), 1, 0) });
    h.take(conn).into_iter().find(|m| matches!(m, ServerMessage::SnapshotResult { .. })).expect("a result")
}

#[test]
fn an_empty_room_takes_one_seed_and_a_joiner_gets_it() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    assert!(matches!(
        seed(&mut h, a, 1),
        ServerMessage::SnapshotResult { ok: true, covers_seq: 0, snapshot_seq: 0, head_seq: 0 }
    ));
    let b = h.join(client(2), JoinMode::Open, 0);
    let messages = h.take(b);
    assert!(
        matches!(catchups(&messages)[0], ServerMessage::Catchup { snapshot: Some(s), .. } if s.covers_seq == 0),
        "{messages:?}"
    );
}

#[test]
fn the_second_seed_loses_and_so_does_a_seed_after_an_append() {
    let mut h = Harness::self_host();
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    assert!(matches!(seed(&mut h, a, 1), ServerMessage::SnapshotResult { ok: true, .. }));
    h.advance(11 * SECOND);
    assert!(matches!(seed(&mut h, b, 2), ServerMessage::SnapshotResult { ok: false, .. }));
    let mut other = Harness::self_host();
    let (c, d) = (other.open(1), other.open(2));
    other.take(c);
    other.take(d);
    other.send(c, &ClientMessage::Append { record: update(client(1), 1) });
    assert!(
        matches!(seed(&mut other, d, 2), ServerMessage::SnapshotResult { ok: false, head_seq: 1, .. }),
        "an append made the room non-empty"
    );
}

#[test]
fn a_seeded_room_compacts_like_any_other_and_a_joiner_never_loops_on_the_seed() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    seed(&mut h, a, 1);
    for ctr in 2..=70 {
        h.send(a, &ClientMessage::Append { record: update(client(1), ctr) });
        h.advance(100);
    }
    h.advance(2 * SECOND);
    let b = h.join(client(2), JoinMode::Open, 0);
    let first = h.take(b);
    assert!(matches!(catchups(&first)[0], ServerMessage::Catchup { snapshot: Some(_), more: true, .. }));
    h.advance(2 * SECOND);
    h.send(b, &ClientMessage::Catchup { after_seq: 64 });
    let second = h.take(b);
    assert!(matches!(catchups(&second)[0], ServerMessage::Catchup { snapshot: None, more: false, .. }));
}
