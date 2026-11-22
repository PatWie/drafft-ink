#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Every input is hostile: random frames, mutated frames, text and unknown connections never panic and never leave
//! the relay in a state that serves the next person wrongly.

mod common;

use common::*;
use drafftink_protocol::v2::{ClientMessage, JoinMode, RecordKind, ServerMessage};
use drafftink_relay::ConnId;
use proptest::prelude::*;

fn valid_frames() -> Vec<Vec<u8>> {
    let a = client(1);
    [
        join_message(a, JoinMode::Create, 0),
        ClientMessage::Append { record: update(a, 1) },
        ClientMessage::PutSnapshot { expected_snapshot_seq: 0, record: snapshot(a, 2, 0) },
        ClientMessage::Presence { record: presence(a) },
        ClientMessage::Catchup { after_seq: 0 },
        ClientMessage::Direct { to: None, record: record(RecordKind::Direct, a, 3, 0, 5) },
        ClientMessage::DeleteRoom,
        ClientMessage::Leave,
    ]
    .iter()
    .map(|m| m.encode().expect("encodes"))
    .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn random_bytes_never_panic_and_never_break_a_later_join(frames in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..300), 1..8)) {
        let mut h = Harness::self_host();
        for frame in &frames {
            let conn = h.connect();
            h.send_raw(conn, frame);
        }
        let good = h.join(client(9), JoinMode::Create, 0);
        let messages = h.take(good);
        prop_assert!(matches!(messages.first(), Some(ServerMessage::Joined { .. })), "{messages:?}");
    }

    #[test]
    fn mutated_valid_frames_never_panic(index in 0usize..8, flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..6), cut in any::<usize>()) {
        let mut h = Harness::self_host();
        let conn = h.open(1);
        h.take(conn);
        let mut frame = valid_frames()[index].clone();
        for (at, value) in flips {
            let len = frame.len();
            frame[at % len] ^= value;
        }
        let keep = cut % (frame.len() + 1);
        frame.truncate(keep.max(1));
        h.send_raw(conn, &frame);
        h.advance(3 * SECOND);
        let other = h.join(client(2), JoinMode::Open, 0);
        let _ = h.take(other);
    }
}

#[test]
fn every_prefix_of_every_valid_frame_is_survived() {
    let mut h = Harness::self_host();
    for frame in valid_frames() {
        for len in 0..frame.len() {
            let conn = h.connect();
            h.send_raw(conn, &frame[..len]);
        }
    }
}

#[test]
fn frames_before_a_join_and_frames_for_unknown_connections_are_ignored_or_refused() {
    let mut h = Harness::self_host();
    for frame in valid_frames().into_iter().skip(1) {
        let conn = h.connect();
        h.send_raw(conn, &frame);
        assert!(h.is_closed(conn));
    }
    let out = h.relay.on_frame(ConnId(999), h.now, &valid_frames()[0]);
    assert!(out.effects.is_empty());
    let out = h.relay.on_text(ConnId(999), h.now, "ping");
    assert!(out.effects.is_empty());
    let _ = h.relay.on_closed(ConnId(999), h.now);
}

#[test]
fn an_oversize_frame_is_refused_with_too_large() {
    let mut cfg = demo();
    cfg.limits.max_frame_bytes = 4096;
    cfg.limits.max_record_bytes = 2048;
    let mut h = Harness::new(cfg);
    let conn = h.open(1);
    h.take(conn);
    h.send_raw(conn, &vec![0x10; 5000]);
    assert_eq!(error_code(&h.take(conn)), Some(drafftink_protocol::v2::ErrorCode::TooLarge));
    assert!(h.is_closed(conn));
}

#[test]
fn a_clock_that_goes_backwards_does_not_panic() {
    let mut h = Harness::new(demo());
    let a = h.open(1);
    h.take(a);
    h.now -= 500_000;
    h.send(a, &ClientMessage::Append { record: update(client(1), 1) });
    let _ = h.relay.on_wake(0);
    let _ = h.relay.on_wake(u64::MAX);
}
