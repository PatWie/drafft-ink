#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Sequence numbers, the segment buffer and its flush rules, fan-out, catch-up and restarts.

mod common;

use common::*;
use drafftink_protocol::v2::{ClientMessage, ErrorCode, JoinMode, RecordKind, ServerMessage};
use drafftink_relay::{RelayConfig, Retention, StoreOp};

fn append(h: &mut Harness, conn: drafftink_relay::ConnId, sender: u8, ctr: u64) {
    h.send(conn, &ClientMessage::Append { record: update(client(sender), ctr) });
}

#[test]
fn records_get_consecutive_sequence_numbers_and_reach_the_others_not_the_sender() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    let b = h.open(2);
    h.take(a);
    h.take(b);
    append(&mut h, a, 1, 1);
    append(&mut h, b, 2, 1);
    append(&mut h, a, 1, 2);
    let to_b = records(&h.take(b));
    let to_a = records(&h.take(a));
    assert_eq!(to_b.iter().map(|r| (r.seq, r.ctr)).collect::<Vec<_>>(), [(1, 1), (3, 2)]);
    assert_eq!(to_a.iter().map(|r| (r.seq, r.ctr)).collect::<Vec<_>>(), [(2, 1)]);
}

#[test]
fn an_ack_comes_only_after_the_flush_and_says_stored() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    append(&mut h, a, 1, 1);
    assert!(acks(&h.take(a)).is_empty(), "not stored yet");
    assert_eq!(h.store.segment_count(), 0);
    h.advance(999);
    assert!(acks(&h.take(a)).is_empty());
    h.advance(2);
    assert_eq!(acks(&h.take(a)), [(1, 1)]);
    assert_eq!(h.store.segment_count(), 1);
}

#[test]
fn records_of_one_flush_window_share_one_segment() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    for ctr in 1..=20 {
        append(&mut h, a, 1, ctr);
        h.advance(40);
    }
    h.advance(2 * SECOND);
    assert_eq!(h.store.segments_written(), 1, "twenty records over 800 ms are one write");
    assert_eq!(acks(&h.take(a)).len(), 20);
    assert!(h.ops.iter().any(|op| matches!(op, StoreOp::AppendSegment { records: 20, .. })));
}

#[test]
fn a_big_buffer_is_written_at_once() {
    let mut cfg = demo();
    cfg.retention = Retention::Never;
    cfg.flush.max_bytes = 1000;
    let mut h = Harness::new(cfg);
    let a = h.open(1);
    h.take(a);
    h.send(a, &ClientMessage::Append { record: record(RecordKind::Update, client(1), 1, 0, 2000) });
    assert_eq!(acks(&h.take(a)), [(1, 1)], "stored without waiting for the timer");
}

#[test]
fn flush_interval_zero_writes_before_every_ack() {
    let mut cfg = demo();
    cfg.retention = Retention::Never;
    cfg.flush.interval_ms = 0;
    let mut h = Harness::new(cfg);
    let a = h.open(1);
    h.take(a);
    append(&mut h, a, 1, 1);
    append(&mut h, a, 1, 2);
    assert_eq!(acks(&h.take(a)), [(1, 1), (2, 2)]);
    assert_eq!(h.store.segments_written(), 2);
}

#[test]
fn a_failed_write_keeps_the_records_and_is_retried_without_an_ack_before_it_works() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    append(&mut h, a, 1, 1);
    h.store.fail_next(1);
    h.advance(1_100);
    assert!(acks(&h.take(a)).is_empty(), "the write failed");
    h.advance(1_100);
    assert_eq!(acks(&h.take(a)), [(1, 1)], "the retry stored it");
    assert_eq!(h.store.segment_count(), 1);
}

#[test]
fn a_sender_that_is_not_the_connection_is_refused() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    append(&mut h, a, 7, 1);
    assert_eq!(error_code(&h.take(a)), Some(ErrorCode::Forbidden));
    assert!(h.is_closed(a));
}

#[test]
fn a_late_joiner_gets_the_log_in_order_and_a_returning_one_only_what_is_new() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    for ctr in 1..=5 {
        append(&mut h, a, 1, ctr);
    }
    let late = h.join(client(2), JoinMode::Open, 0);
    let messages = h.take(late);
    assert_eq!(catchup_seqs(&messages), [1, 2, 3, 4, 5], "the join flushed the buffer first");
    let back = h.join(client(3), JoinMode::Open, 3);
    assert_eq!(catchup_seqs(&h.take(back)), [4, 5]);
}

#[test]
fn catchup_pages_stop_at_64_records_and_say_more() {
    let mut cfg = demo();
    cfg.retention = Retention::Never;
    cfg.max_room_updates_per_sec = 1000;
    cfg.limits.max_updates_per_sec = 1000;
    cfg.compact.records = 10_000;
    let mut h = Harness::new(cfg);
    let a = h.open(1);
    h.take(a);
    for ctr in 1..=150 {
        append(&mut h, a, 1, ctr);
    }
    let late = h.join(client(2), JoinMode::Open, 0);
    let first = h.take(late);
    let Some(ServerMessage::Catchup { records, more, head_seq, .. }) = catchups(&first).first().copied().cloned()
    else {
        panic!("{first:?}")
    };
    assert_eq!((records.len(), more, head_seq), (64, true, 150));
    h.advance(2 * SECOND);
    h.send(late, &ClientMessage::Catchup { after_seq: 64 });
    h.advance(2 * SECOND);
    h.send(late, &ClientMessage::Catchup { after_seq: 128 });
    let rest = h.take(late);
    let pages: Vec<_> = catchups(&rest)
        .into_iter()
        .filter_map(|m| {
            if let ServerMessage::Catchup { records, more, .. } = m { Some((records.len(), *more)) } else { None }
        })
        .collect();
    assert_eq!(pages, [(64, true), (22, false)]);
}

#[test]
fn catchup_requests_are_limited_to_one_a_second_with_a_burst_of_four() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    for _ in 0..4 {
        h.send(a, &ClientMessage::Catchup { after_seq: 0 });
    }
    assert_eq!(catchups(&h.take(a)).len(), 4, "a long catch-up may ask for its pages one after the other");
    h.send(a, &ClientMessage::Catchup { after_seq: 0 });
    let messages = h.take(a);
    assert_eq!(error_code(&messages), Some(ErrorCode::RateLimited));
    assert!(matches!(messages[0], ServerMessage::Error { retry_after_ms: Some(_), .. }));
    h.advance(1100);
    h.send(a, &ClientMessage::Catchup { after_seq: 0 });
    assert_eq!(catchups(&h.take(a)).len(), 1, "one more a second later");
}

#[test]
fn a_restarted_relay_serves_what_the_first_one_stored() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    append(&mut h, a, 1, 1);
    append(&mut h, a, 1, 2);
    h.advance(2 * SECOND);
    let (store, blobs) = (h.store.clone(), h.blobs.clone());
    let mut restarted = Harness::on(RelayConfig { retention: Retention::Never, ..demo() }, store, blobs);
    let b = restarted.join(client(2), JoinMode::Open, 0);
    let messages = restarted.take(b);
    assert!(
        matches!(
            &messages[0],
            ServerMessage::Joined { head_seq: 2, room_state: drafftink_protocol::v2::RoomState::Existing, .. }
        ),
        "{messages:?}"
    );
    assert_eq!(catchup_seqs(&messages), [1, 2]);
    append(&mut restarted, b, 2, 1);
    restarted.advance(2 * SECOND);
    assert_eq!(acks(&restarted.take(b)), [(1, 3)], "sequence numbers go on where they stopped");
}

#[test]
fn updates_over_the_rate_are_dropped_with_a_wait_and_a_flood_closes_the_connection() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    for ctr in 1..=200 {
        append(&mut h, a, 1, ctr);
    }
    let messages = h.take(a);
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, ServerMessage::Error { code: ErrorCode::RateLimited, retry_after_ms: Some(_), .. }))
    );
    assert!(h.is_closed(a), "fifty breaches in a row end the connection");
}

#[test]
fn a_record_over_the_limit_or_a_full_room_is_refused() {
    let mut cfg = demo();
    cfg.retention = Retention::Never;
    cfg.limits.max_record_bytes = 1000;
    cfg.max_room_state_bytes = 5_000;
    let mut h = Harness::new(cfg);
    let a = h.open(1);
    h.take(a);
    h.send(a, &ClientMessage::Append { record: record(RecordKind::Update, client(1), 1, 0, 2_000) });
    assert_eq!(error_code(&h.take(a)), Some(ErrorCode::TooLarge));
    assert!(!h.is_closed(a));
    for ctr in 1..=10 {
        h.send(a, &ClientMessage::Append { record: record(RecordKind::Update, client(1), ctr, 0, 500) });
    }
    assert!(h.take(a).iter().any(|m| matches!(m, ServerMessage::Error { code: ErrorCode::RoomFull, .. })));
}

#[test]
fn direct_messages_reach_the_target_or_everybody_else() {
    let mut h = Harness::self_host();
    let (a, b, c) = (h.open(1), h.open(2), h.open(3));
    for conn in [a, b, c] {
        h.take(conn);
    }
    let direct = record(RecordKind::Direct, client(1), 1, 0, 8);
    h.send(a, &ClientMessage::Direct { to: Some(client(2)), record: direct.clone() });
    assert!(matches!(&h.take(b)[..], [ServerMessage::Direct { from, .. }] if *from == client(1)));
    assert!(h.take(c).is_empty());
    h.send(a, &ClientMessage::Direct { to: None, record: direct });
    assert_eq!((h.take(b).len(), h.take(c).len(), h.take(a).len()), (1, 1, 0));
}

#[test]
fn leave_closes_and_tells_the_others() {
    let mut h = Harness::self_host();
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    h.send(a, &ClientMessage::Leave);
    assert!(h.is_closed(a));
    assert!(matches!(h.take(b)[0], ServerMessage::PeerLeft { .. }));
}
