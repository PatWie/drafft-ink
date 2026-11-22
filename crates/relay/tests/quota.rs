#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The daily limit of a host: a store that fails with `DailyLimit` is told to people with the time of the reset,
//! sheds the room instead of retrying every second, and is reported to the host in `Output::quota_hit`.

mod common;

use common::*;
use drafftink_protocol::v2::{ClientMessage, ErrorCode, ImageName, JoinMode, RecordKind, ServerMessage};
use drafftink_protocol::{ClientId, SEAL_OVERHEAD};
use drafftink_relay::{ImageBody, ImageOp, ImageRequest, ImageResponse, ImageStatus, ms_until_utc_midnight};

fn daily_limit(messages: &[ServerMessage]) -> Option<Option<u32>> {
    messages.iter().find_map(|m| match m {
        ServerMessage::Error { code: ErrorCode::DailyLimit, retry_after_ms, .. } => Some(*retry_after_ms),
        _ => None,
    })
}

fn image_request(h: &mut Harness, op: ImageOp<'_>) -> (ImageResponse, bool) {
    let (response, out) = h.relay.image_request(h.now, ImageRequest { proof: &proof(), op });
    (response, out.quota_hit)
}

fn name() -> ImageName {
    ImageName::from_bytes([7; 16])
}

fn sealed_image() -> Vec<u8> {
    record(RecordKind::Image, ClientId::from_bytes([0; 16]), 0, 0, 100 - SEAL_OVERHEAD)
}

#[test]
fn a_join_on_a_full_day_is_refused_with_the_reset_time_and_closed() {
    let mut h = Harness::self_host();
    h.store.set_daily_limit(true);
    let a = h.open(1);
    let frames = h.take(a);
    assert_eq!(daily_limit(&frames), Some(Some(ms_until_utc_midnight(h.now))));
    assert!(h.is_closed(a));
    assert!(h.quota_hit);
    assert_eq!(h.relay.connection_count(), 0);
    let text =
        frames.iter().find_map(|m| if let ServerMessage::Error { message, .. } = m { Some(message) } else { None });
    assert!(text.unwrap().contains("00:00 UTC"), "{text:?}");
}

#[test]
fn the_retry_time_is_counted_from_the_time_of_the_call() {
    let mut h = Harness::self_host();
    h.store.set_daily_limit(true);
    h.now = 3 * 86_400_000 + 86_399_000;
    let a = h.open(1);
    assert_eq!(daily_limit(&h.take(a)), Some(Some(1000)));
}

#[test]
fn another_store_failure_is_still_unavailable_and_is_not_a_quota_hit() {
    let mut h = Harness::self_host();
    h.store.fail_next(1);
    let a = h.open(1);
    assert_eq!(error_code(&h.take(a)), Some(ErrorCode::Unavailable));
    assert!(!h.quota_hit);
    h.blobs.fail_next(1);
    let b = h.open(2);
    h.take(b);
    let (response, hit) = image_request(&mut h, ImageOp::List);
    assert_eq!((response.status, hit), (ImageStatus::Unavailable, false));
    assert_eq!(response.retry_after_ms, None);
}

#[test]
fn a_flush_that_hits_the_limit_sheds_everyone_and_stops_retrying() {
    let mut h = Harness::self_host();
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    h.send(a, &ClientMessage::Append { record: update(client(1), 1) });
    assert_eq!(records(&h.take(b)).len(), 1, "b saw sequence number 1");
    h.store.set_daily_limit(true);
    let flush_at = h.now + h.relay.config().flush.interval_ms;
    h.advance(2 * SECOND);

    assert!(h.quota_hit);
    for conn in [a, b] {
        assert_eq!(daily_limit(&h.take(conn)), Some(Some(ms_until_utc_midnight(flush_at))));
        assert!(h.is_closed(conn));
    }
    assert_eq!(h.relay.connection_count(), 0);
    assert_eq!(h.wake_at, None, "no retry timer: the host goes idle");
    h.quota_hit = false;
    h.advance(10 * SECOND);
    assert!(!h.quota_hit, "nothing is written again");
    assert!(h.take(a).is_empty());

    // After the reset: the unacknowledged update was dropped. b rejoins above the head and gets a fresh room.
    h.store.set_daily_limit(false);
    let b2 = h.join(client(2), JoinMode::Open, 1);
    let frames = h.take(b2);
    assert!(frames.iter().any(|m| matches!(m, ServerMessage::Joined { head_seq: 0, .. })));
    assert!(catchup_seqs(&frames).is_empty());
    let a2 = h.join(client(1), JoinMode::Open, 0);
    h.take(a2);
    h.send(a2, &ClientMessage::Append { record: update(client(1), 1) });
    h.advance(2 * SECOND);
    assert_eq!(acks(&h.take(a2)), [(1, 1)], "the client resent its update");
}

#[test]
fn an_inline_flush_by_size_sheds_too() {
    let mut cfg = demo();
    cfg.flush.max_bytes = 1;
    let mut h = Harness::new(cfg);
    let a = h.open(1);
    h.take(a);
    h.store.set_daily_limit(true);
    h.send(a, &ClientMessage::Append { record: update(client(1), 1) });
    assert!(h.quota_hit);
    assert!(daily_limit(&h.take(a)).is_some());
    assert!(h.is_closed(a));
}

#[test]
fn a_catch_up_and_a_snapshot_under_the_limit_say_so() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    h.send(a, &ClientMessage::Append { record: update(client(1), 1) });
    h.advance(2 * SECOND);
    h.take(a);
    h.store.set_daily_limit(true);
    h.send(a, &ClientMessage::Catchup { after_seq: 0 });
    assert!(h.quota_hit);
    assert!(daily_limit(&h.take(a)).is_some());
    assert!(h.is_closed(a));
}

#[test]
fn a_failure_without_a_frame_still_sets_quota_hit() {
    // A retention check with nobody connected reads the room: the call sends nothing, and the host still hears.
    let mut h = Harness::new(demo());
    let a = h.open(1);
    h.take(a);
    h.close(a);
    let mut h = Harness::wake_from(h);
    h.store.set_daily_limit(true);
    h.tick();
    assert!(h.quota_hit);
    assert!(h.inbox.is_empty() && h.closed.is_empty());
}

#[test]
fn a_deletion_that_hits_the_limit_waits_for_the_reset() {
    let mut h = Harness::new(demo());
    let a = h.open(1);
    h.take(a);
    h.close(a);
    h.advance(48 * HOUR - 10 * SECOND);
    h.store.set_daily_limit(true);
    h.advance(20 * SECOND + 61 * SECOND);
    assert!(h.quota_hit && !h.deleted);
    let retry = h.wake_at.unwrap();
    assert!(retry > h.now + 20 * HOUR, "the next try is at the reset (about 23 h away), not in a second");
    h.store.set_daily_limit(false);
    h.advance(retry - h.now);
    assert!(h.deleted);
}

#[test]
fn the_image_routes_map_the_limit_to_503_with_the_reset_time() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    let body = sealed_image();
    let (stored, _) = image_request(&mut h, ImageOp::Put { name: name(), body: &body });
    assert_eq!(stored.status, ImageStatus::Created);

    h.blobs.set_daily_limit(true);
    let wait = Some(ms_until_utc_midnight(h.now));
    for op in [ImageOp::Put { name: name(), body: &body }, ImageOp::Get { name: name() }, ImageOp::List] {
        let (response, hit) = image_request(&mut h, op);
        assert_eq!(response.status, ImageStatus::DailyLimit);
        assert_eq!((response.status.http(), response.status.code()), (503, "daily_limit"));
        assert_eq!((response.retry_after_ms, response.body, hit), (wait, ImageBody::Empty, true));
    }
    h.blobs.set_daily_limit(false);
    assert_eq!(image_request(&mut h, ImageOp::List).0.status, ImageStatus::Ok);
}

#[test]
fn the_image_routes_map_a_limit_on_the_room_store_too() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    let mut woken = Harness::wake_from(h);
    woken.store.set_daily_limit(true);
    let (response, hit) = image_request(&mut woken, ImageOp::List);
    assert_eq!((response.status, hit), (ImageStatus::DailyLimit, true));
    assert_eq!(response.retry_after_ms, Some(ms_until_utc_midnight(woken.now)));
}
