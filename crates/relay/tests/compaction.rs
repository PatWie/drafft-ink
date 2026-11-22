#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Compaction by compare-and-set: who is asked, who wins a race, and what a joiner gets afterwards.

mod common;

use common::*;
use drafftink_protocol::v2::{ClientMessage, ErrorCode, JoinMode, RecordKind, ServerMessage};
use drafftink_relay::{ConnId, Retention};

fn small_log() -> drafftink_relay::RelayConfig {
    let mut cfg = demo();
    cfg.retention = Retention::Never;
    cfg.flush.interval_ms = 0;
    cfg.compact.records = 5;
    cfg.compact.join_records = 3;
    cfg.max_room_updates_per_sec = 1000;
    cfg.limits.max_updates_per_sec = 1000;
    cfg
}

fn fill(h: &mut Harness, conn: ConnId, sender: u8, from_ctr: u64, count: u64) {
    for ctr in from_ctr..from_ctr + count {
        h.send(conn, &ClientMessage::Append { record: update(client(sender), ctr) });
    }
}

fn advice(messages: &[ServerMessage]) -> Vec<u64> {
    messages
        .iter()
        .filter_map(|m| if let ServerMessage::CompactAdvice { head_seq } = m { Some(*head_seq) } else { None })
        .collect()
}

fn put(h: &mut Harness, conn: ConnId, sender: u8, expected: u64, covers: u64) -> ServerMessage {
    h.send(
        conn,
        &ClientMessage::PutSnapshot {
            expected_snapshot_seq: expected,
            record: snapshot(client(sender), 100 + covers, covers),
        },
    );
    h.take(conn).into_iter().find(|m| matches!(m, ServerMessage::SnapshotResult { .. })).expect("a result")
}

#[test]
fn advice_goes_to_one_person_the_longest_connected() {
    let mut h = Harness::new(small_log());
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    fill(&mut h, a, 1, 1, 5);
    assert_eq!(advice(&h.take(a)), [5], "the first to join is asked");
    assert!(advice(&h.take(b)).is_empty());
    fill(&mut h, a, 1, 6, 2);
    assert!(advice(&h.take(a)).is_empty() && advice(&h.take(b)).is_empty(), "one is asked at a time");
}

#[test]
fn advice_that_gets_no_answer_in_thirty_seconds_goes_to_the_next_person() {
    let mut h = Harness::new(small_log());
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    fill(&mut h, a, 1, 1, 5);
    h.take(a);
    h.advance(31 * SECOND);
    assert_eq!(advice(&h.take(b)), [5]);
    assert!(advice(&h.take(a)).is_empty());
}

#[test]
fn a_newcomer_who_applied_a_long_log_is_asked_to_compact() {
    let mut cfg = small_log();
    cfg.compact.records = 1000;
    let mut h = Harness::new(cfg);
    let a = h.open(1);
    h.take(a);
    fill(&mut h, a, 1, 1, 4);
    let late = h.join(client(2), JoinMode::Open, 0);
    assert_eq!(advice(&h.take(late)), [4]);
}

#[test]
fn a_snapshot_replaces_the_log_it_covers_and_a_joiner_converges_from_it() {
    let mut h = Harness::new(small_log());
    let a = h.open(1);
    h.take(a);
    fill(&mut h, a, 1, 1, 8);
    h.take(a);
    let result = put(&mut h, a, 1, 0, 6);
    assert!(
        matches!(result, ServerMessage::SnapshotResult { ok: true, covers_seq: 6, snapshot_seq: 6, head_seq: 8 }),
        "{result:?}"
    );
    let late = h.join(client(2), JoinMode::Open, 0);
    let messages = h.take(late);
    let Some(ServerMessage::Catchup { snapshot: Some(s), records, .. }) = catchups(&messages).first().copied().cloned()
    else {
        panic!("{messages:?}")
    };
    assert_eq!(s.covers_seq, 6);
    assert_eq!(records.iter().map(|r| r.seq).collect::<Vec<_>>(), [7, 8]);
    let back = h.join(client(3), JoinMode::Open, 8);
    let messages = h.take(back);
    assert!(
        matches!(catchups(&messages)[0], ServerMessage::Catchup { snapshot: None, .. }),
        "a client past the cover needs no snapshot"
    );
    let behind = h.join(client(4), JoinMode::Open, 3);
    assert!(
        matches!(
            h.take(behind).iter().find(|m| matches!(m, ServerMessage::Catchup { .. })),
            Some(ServerMessage::Catchup { snapshot: Some(_), .. })
        ),
        "a client behind the cover gets it"
    );
}

#[test]
fn two_racing_compactions_one_wins_and_no_live_record_is_lost() {
    let mut h = Harness::new(small_log());
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    fill(&mut h, a, 1, 1, 6);
    h.take(a);
    h.take(b);
    let first = put(&mut h, a, 1, 0, 5);
    let second = put(&mut h, b, 2, 0, 6);
    assert!(matches!(first, ServerMessage::SnapshotResult { ok: true, .. }));
    assert!(
        matches!(second, ServerMessage::SnapshotResult { ok: false, snapshot_seq: 5, head_seq: 6, .. }),
        "{second:?}"
    );
    let late = h.join(client(3), JoinMode::Open, 0);
    let messages = h.take(late);
    assert_eq!(catchup_seqs(&messages), [6], "record 6 is still there");
}

#[test]
fn a_stale_expectation_or_a_cover_beyond_the_head_is_refused_and_changes_nothing() {
    let mut h = Harness::new(small_log());
    let a = h.open(1);
    h.take(a);
    fill(&mut h, a, 1, 1, 6);
    h.take(a);
    assert!(matches!(put(&mut h, a, 1, 0, 6), ServerMessage::SnapshotResult { ok: true, .. }));
    h.advance(11 * SECOND);
    assert!(matches!(put(&mut h, a, 1, 0, 6), ServerMessage::SnapshotResult { ok: false, .. }), "stale expectation");
    h.advance(11 * SECOND);
    assert!(matches!(put(&mut h, a, 1, 6, 9), ServerMessage::SnapshotResult { ok: false, .. }), "beyond the head");
    h.advance(11 * SECOND);
    assert!(matches!(put(&mut h, a, 1, 6, 6), ServerMessage::SnapshotResult { ok: false, .. }), "covers nothing new");
}

#[test]
fn records_below_the_cover_are_deleted_in_the_same_transaction() {
    let mut h = Harness::new(small_log());
    let a = h.open(1);
    h.take(a);
    fill(&mut h, a, 1, 1, 4);
    assert_eq!(h.store.segment_count(), 4);
    h.take(a);
    put(&mut h, a, 1, 0, 3);
    assert_eq!(h.store.segment_count(), 1, "segments 1 to 3 went with the snapshot");
    assert!(h.ops.iter().any(|op| matches!(op, drafftink_relay::StoreOp::PutSnapshot { covers_seq: 3, .. })));
}

#[test]
fn snapshots_may_be_sent_once_every_ten_seconds() {
    let mut h = Harness::new(small_log());
    let a = h.open(1);
    h.take(a);
    fill(&mut h, a, 1, 1, 6);
    h.take(a);
    assert!(matches!(put(&mut h, a, 1, 0, 3), ServerMessage::SnapshotResult { ok: true, .. }));
    h.send(a, &ClientMessage::PutSnapshot { expected_snapshot_seq: 3, record: snapshot(client(1), 200, 5) });
    assert_eq!(error_code(&h.take(a)), Some(ErrorCode::RateLimited));
}

#[test]
fn a_snapshot_of_another_sender_is_refused() {
    let mut h = Harness::new(small_log());
    let a = h.open(1);
    h.take(a);
    h.send(
        a,
        &ClientMessage::PutSnapshot {
            expected_snapshot_seq: 0,
            record: record(RecordKind::Snapshot, client(9), 1, 0, 10),
        },
    );
    assert_eq!(error_code(&h.take(a)), Some(ErrorCode::Forbidden));
}
