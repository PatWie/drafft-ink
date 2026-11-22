//! The relay over the host stores, with the frames that the TypeScript tests use (`test/fixtures/frames.json`).
//! These tests run the same record, snapshot and catch-up code paths as the Durable Object, over [`FakeHost`].

use drafftink_protocol::v2::{ErrorCode, RoomId, ServerMessage};
use serde_json::Value;

use crate::config::relay_config;
use crate::driver::{Driver, Effects, ImageCall, Item};
use crate::fake::FakeHost;
use crate::host::{HostBlobs, HostError, HostStore};

type TestDriver = Driver<HostStore<FakeHost>, HostBlobs<FakeHost>>;

const SECOND: u64 = 1000;
const T0: u64 = 1_700_000_000_000;

fn fixtures() -> Value {
    serde_json::from_str(include_str!("../test/fixtures/frames.json")).unwrap()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2).map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap()).collect()
}

fn frame(name: &str) -> Vec<u8> {
    unhex(fixtures()["frames"][name].as_str().unwrap())
}

/// The `Authorization` header of a member.
fn token() -> String {
    format!("{} {}", crate::edge::AUTH_SCHEME, fixtures()["room"]["token"]["base64urlNoPad"].as_str().unwrap())
}

fn room() -> RoomId {
    fixtures()["room"]["id"].as_str().unwrap().parse().unwrap()
}

fn driver_over(host: &FakeHost, settings: &str) -> TestDriver {
    Driver::new(room(), relay_config(settings).unwrap(), HostStore::new(host.clone()), HostBlobs::new(host.clone()))
}

/// What the frames of `effects` say, for one connection.
fn messages(effects: &Effects, conn: u32) -> Vec<ServerMessage> {
    effects
        .items()
        .iter()
        .filter_map(|item| match item {
            Item::Send { conn: c, frame } if *c == conn => {
                Some(ServerMessage::decode(effects.frame(*frame).unwrap()).unwrap())
            }
            _ => None,
        })
        .collect()
}

fn closes(effects: &Effects, conn: u32) -> bool {
    effects.items().contains(&Item::Close { conn })
}

fn has<F: Fn(&ServerMessage) -> bool>(effects: &Effects, conn: u32, f: F) -> bool {
    messages(effects, conn).iter().any(f)
}

/// Opens connection `conn` and joins it with `join`.
fn join(driver: &mut TestDriver, conn: u32, now: u64, join: &str) -> Effects {
    let _ = driver.connect(conn, now);
    driver.on_frame(conn, now, &frame(join))
}

#[test]
fn records_are_acknowledged_after_the_flush_and_reach_the_peer() {
    let host = FakeHost::default();
    let mut driver = driver_over(&host, "{}");
    let joined = join(&mut driver, 1, T0, "joinCreateA");
    assert!(has(&joined, 1, |m| matches!(m, ServerMessage::Joined { .. })));
    join(&mut driver, 2, T0, "joinOpenB");
    let appended = driver.on_frame(1, T0 + 10, &frame("appendA1"));
    assert!(has(&appended, 2, |m| matches!(m, ServerMessage::Record(r) if r.seq == 1)), "fan-out is at once");
    assert!(!has(&appended, 1, |m| matches!(m, ServerMessage::Ack { .. })), "no Ack before the write");
    let flushed = driver.on_wake(T0 + 2 * SECOND);
    assert!(has(&flushed, 1, |m| matches!(m, ServerMessage::Ack { seq: 1, .. })));
    assert_eq!(host.segment_rows(), 1, "one row per flush");
    // The rows hold sealed bytes: the relay never saw the plaintext of the fixture.
    let marker = fixtures()["marker"].as_str().unwrap().as_bytes().to_vec();
    assert!(host.dump().iter().all(|row| !row.windows(marker.len()).any(|w| w == marker.as_slice())));
}

#[test]
fn a_core_rebuilt_over_the_same_rows_serves_catch_up_and_takes_restored_members() {
    let host = FakeHost::default();
    let mut first = driver_over(&host, "{}");
    join(&mut first, 1, T0, "joinCreateA");
    join(&mut first, 2, T0, "joinOpenB");
    first.on_frame(1, T0 + 10, &frame("appendA1"));
    first.on_wake(T0 + 2 * SECOND);
    first.on_frame(2, T0 + 3 * SECOND, &frame("appendB1"));
    first.on_wake(T0 + 5 * SECOND);
    assert_eq!(host.segment_rows(), 2);
    let ids = (first.member_of(1).unwrap(), first.member_of(2).unwrap());
    drop(first);

    // The object hibernated and woke up: a new core, the sockets restored from their attachments.
    let mut second = driver_over(&host, "{}");
    let wake = T0 + 60 * SECOND;
    second.restore_member(1, ids.0.0, ids.0.1, wake, wake);
    second.restore_member(2, ids.1.0, ids.1.1, wake, wake);
    assert_eq!(second.relay().member_count(), 2);
    let appended = second.on_frame(1, wake, &frame("appendA2"));
    assert!(
        has(&appended, 2, |m| matches!(m, ServerMessage::Record(r) if r.seq == 3)),
        "sequence continues from the store"
    );
    // A newcomer is caught up over both old segments.
    let late = join(&mut second, 3, wake + 10, "joinOpenC");
    assert!(late.items().iter().any(|i| matches!(i, Item::Send { conn: 3, .. })));
    let catchup = messages(&late, 3).into_iter().find_map(|m| match m {
        ServerMessage::Catchup { records, .. } => Some(records),
        _ => None,
    });
    let seqs: Vec<u64> = catchup.unwrap().iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3], "the two old rows, then the new record that a join writes first");
}

#[test]
fn a_snapshot_replaces_the_covered_segments_in_one_swap() {
    let host = FakeHost::default();
    let mut driver = driver_over(&host, "{}");
    join(&mut driver, 1, T0, "joinCreateA");
    driver.on_frame(1, T0 + 10, &frame("appendA1"));
    driver.on_frame(1, T0 + 20, &frame("appendA2"));
    let swapped = driver.on_frame(1, T0 + 30, &frame("snapshotCovers2"));
    assert!(
        has(&swapped, 1, |m| matches!(m, ServerMessage::SnapshotResult { ok: true, covers_seq: 2, .. })),
        "{swapped:?}"
    );
    assert_eq!(host.segment_rows(), 0, "the segment is covered");
    assert_eq!(host.calls("put_snapshot"), 1);
    let rejoin = join(&mut driver, 2, T0 + 40, "joinOpenB");
    let catchup = messages(&rejoin, 2).into_iter().find_map(|m| match m {
        ServerMessage::Catchup { snapshot, records, .. } => Some((snapshot, records)),
        _ => None,
    });
    let (snapshot, records) = catchup.unwrap();
    assert_eq!(snapshot.unwrap().covers_seq, 2);
    assert!(records.is_empty());
}

#[test]
fn pictures_are_stored_listed_and_read_back_through_the_core() {
    let host = FakeHost::default();
    let mut driver = driver_over(&host, "{}");
    join(&mut driver, 1, T0, "joinCreateA");
    let image = fixtures()["image"].clone();
    let name = image["name"].as_str().unwrap();
    let sealed = unhex(image["sealedHex"].as_str().unwrap());
    let (put, _) = driver.image(T0, ImageCall::Put, Some(&token()), Some(name), &sealed);
    assert_eq!((put.status, put.code), (201, "created"));
    let (get, _) = driver.image(T0, ImageCall::Get, Some(&token()), Some(name), &[]);
    assert_eq!((get.status, get.body), (200, sealed.clone()));
    let (list, _) = driver.image(T0, ImageCall::List, Some(&token()), None, &[]);
    let listed: Value = serde_json::from_slice(&list.body).unwrap();
    assert_eq!(listed[0]["name"], name);
    assert_eq!(listed[0]["size"], sealed.len());
    let (bad, _) = driver.image(T0, ImageCall::Get, Some(&token()), Some("not a name"), &[]);
    assert_eq!(bad.status, 400);
    let (stranger, _) = driver.image(T0, ImageCall::List, Some("Drafft-Member AAAA"), None, &[]);
    assert_eq!(stranger.status, 401);
}

#[test]
fn a_used_up_daily_limit_is_an_error_frame_and_a_503() {
    let host = FakeHost::default();
    let mut driver = driver_over(&host, "{}");
    host.fail_with(HostError::DailyLimit);
    let refused = join(&mut driver, 1, T0, "joinCreateA");
    assert!(refused.quota_hit());
    assert!(closes(&refused, 1));
    let error = messages(&refused, 1).into_iter().find_map(|m| match m {
        ServerMessage::Error { code, retry_after_ms, .. } => Some((code, retry_after_ms)),
        _ => None,
    });
    let (code, retry) = error.unwrap();
    assert_eq!(code, ErrorCode::DailyLimit);
    assert_eq!(retry.unwrap(), drafftink_relay::ms_until_utc_midnight(T0));
    let (reply, effects) = driver.image(T0, ImageCall::List, Some(&token()), None, &[]);
    assert_eq!((reply.status, reply.code), (503, "daily_limit"));
    assert!(reply.retry_after_ms.is_some());
    assert!(effects.quota_hit());
}

#[test]
fn another_storage_failure_stays_unavailable() {
    let host = FakeHost::default();
    let mut driver = driver_over(&host, "{}");
    host.fail_with(HostError::Failed("disk is full".into()));
    let refused = join(&mut driver, 1, T0, "joinCreateA");
    assert!(!refused.quota_hit());
    assert!(has(&refused, 1, |m| matches!(m, ServerMessage::Error { code: ErrorCode::Unavailable, .. })));
}

#[test]
fn an_abandoned_room_past_its_deadline_is_deleted_on_wake() {
    let host = FakeHost::default();
    let settings = r#"{"ROOM_TTL_SECS": "3", "ROOM_LEAVE_GRACE_SECS": "1"}"#;
    let mut driver = driver_over(&host, settings);
    let joined = join(&mut driver, 1, T0, "joinCreateA");
    assert_eq!(joined.wake_at(), Some(T0 + 3 * SECOND), "the deadline is the next wake");
    driver.on_frame(1, T0 + 1, &frame("leave"));
    // The object slept; a new core wakes at the deadline.
    let mut woken = driver_over(&host, settings);
    let at_deadline = woken.on_wake(T0 + 3 * SECOND + 1);
    assert!(at_deadline.deleted(), "nobody is live, so the room goes");
}
