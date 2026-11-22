#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Retention with simulated time: the deadline is the last join plus the TTL, edits do not extend it, and a room
//! with people in it is deleted only after the last of them leaves.

mod common;

use common::*;
use drafftink_protocol::v2::{ClientMessage, ErrorCode, JoinMode, ServerMessage};
use drafftink_relay::Retention;
use std::time::Duration;

const TTL: u64 = 48 * HOUR;

fn demo_room() -> Harness {
    Harness::new(demo())
}

fn expires_at(messages: &[ServerMessage]) -> Option<u64> {
    messages
        .iter()
        .find_map(|m| if let ServerMessage::Joined { expires_at_ms, .. } = m { Some(*expires_at_ms) } else { None })
}

fn stored_bytes(h: &Harness) -> usize {
    h.store.dump().len()
}

#[test]
fn a_join_sets_the_deadline_and_a_later_join_moves_it() {
    let mut h = demo_room();
    let start = h.now;
    let a = h.open(1);
    assert_eq!(expires_at(&h.take(a)), Some(start + TTL));
    h.close(a);
    h.advance(10 * HOUR);
    let b = h.join(client(2), JoinMode::Open, 0);
    assert_eq!(expires_at(&h.take(b)), Some(start + 10 * HOUR + TTL));
}

#[test]
fn edits_presence_pings_and_image_requests_do_not_move_the_deadline() {
    let mut h = demo_room();
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    h.advance(10 * HOUR);
    h.send(a, &ClientMessage::Append { record: update(client(1), 1) });
    h.send(a, &ClientMessage::Presence { record: presence(client(1)) });
    let _ = h.relay.on_text(a, h.now, "ping");
    let late = h
        .relay
        .image_request(h.now, drafftink_relay::ImageRequest { proof: &proof(), op: drafftink_relay::ImageOp::List });
    assert_eq!(late.0.status, drafftink_relay::ImageStatus::Ok);
    h.close(a);
    h.close(b);
    h.advance(TTL - 10 * HOUR + 2 * SECOND);
    assert!(h.deleted, "the first join's deadline held");
}

#[test]
fn an_alarm_before_the_deadline_changes_nothing() {
    let mut h = demo_room();
    let a = h.open(1);
    h.close(a);
    h.advance(TTL - HOUR);
    assert!(!h.deleted);
    assert_eq!(h.wake_at, Some(1_000_000 + TTL), "the next wake is the deadline");
}

#[test]
fn an_abandoned_room_is_deleted_at_the_deadline_and_storage_is_empty() {
    let mut h = demo_room();
    let a = h.open(1);
    h.send(a, &ClientMessage::Append { record: update(client(1), 1) });
    h.close(a);
    h.advance(2 * SECOND);
    assert!(stored_bytes(&h) > 0);
    h.advance(TTL);
    assert!(h.deleted);
    assert_eq!(stored_bytes(&h), 0);
    assert!(h.store.segment_count() == 0);
    let late = h.join(client(1), JoinMode::Open, 0);
    assert_eq!(error_code(&h.take(late)), Some(ErrorCode::RoomMissing), "an expired room is gone");
    assert_eq!(h.wake_at, None);
}

#[test]
fn someone_connected_at_the_deadline_keeps_the_room_and_it_is_marked_expiring() {
    let mut h = demo_room();
    let (a, b) = (h.open(1), h.open(2));
    h.take(a);
    h.take(b);
    h.pingers = vec![a, b];
    h.advance(TTL - 1);
    for _ in 0..3 {
        h.advance(HOUR);
        assert!(!h.deleted);
    }
    let messages = h.take(a);
    assert!(messages.iter().any(|m| matches!(m, ServerMessage::Retention { expiring: true, .. })), "{messages:?}");
    assert_eq!(
        messages.iter().filter(|m| matches!(m, ServerMessage::Retention { expiring: true, .. })).count(),
        1,
        "told once"
    );
}

#[test]
fn the_last_person_leaving_an_expired_room_deletes_it_after_the_grace_not_before() {
    let mut h = demo_room();
    let a = h.open(1);
    h.take(a);
    h.pingers = vec![a];
    h.advance(TTL);
    assert!(!h.deleted);
    h.pingers.clear();
    h.close(a);
    h.advance(59 * SECOND);
    assert!(!h.deleted);
    h.advance(2 * SECOND);
    assert!(h.deleted);
    assert_eq!(stored_bytes(&h), 0);
}

#[test]
fn a_rejoin_inside_the_grace_keeps_the_room_and_restarts_the_ttl() {
    let mut h = demo_room();
    let a = h.open(1);
    h.take(a);
    h.pingers = vec![a];
    h.advance(TTL);
    h.pingers.clear();
    h.close(a);
    h.advance(30 * SECOND);
    let b = h.join(client(1), JoinMode::Open, 0);
    let now = h.now;
    assert_eq!(expires_at(&h.take(b)), Some(now + TTL));
    h.advance(2 * HOUR);
    assert!(!h.deleted);
    let messages = h.take(b);
    assert!(!messages.iter().any(|m| matches!(m, ServerMessage::Retention { expiring: true, .. })));
}

#[test]
fn a_socket_silent_for_over_two_minutes_does_not_hold_the_room_open() {
    let mut h = demo_room();
    let a = h.open(1);
    h.take(a);
    h.advance(TTL + 3 * 60 * SECOND);
    assert!(h.deleted, "no frame for days: the socket is not live");
    assert!(h.is_closed(a), "the dead socket is closed with the room");
}

#[test]
fn a_socket_that_keeps_pinging_holds_the_room_open() {
    let mut h = demo_room();
    let a = h.open(1);
    h.take(a);
    h.pingers = vec![a];
    h.advance(TTL + 20 * 60 * SECOND);
    assert!(!h.deleted);
}

#[test]
fn a_later_visitor_clears_the_expiring_state_for_everybody() {
    let mut h = demo_room();
    let a = h.open(1);
    h.take(a);
    h.pingers = vec![a];
    h.advance(TTL + SECOND);
    assert!(h.take(a).iter().any(|m| matches!(m, ServerMessage::Retention { expiring: true, .. })));
    let b = h.join(client(2), JoinMode::Open, 0);
    h.take(b);
    let told = h.take(a);
    assert!(
        told.iter().any(
            |m| matches!(m, ServerMessage::Retention { expiring: false, expires_at_ms } if *expires_at_ms > h.now)
        ),
        "{told:?}"
    );
}

#[test]
fn a_room_that_never_expires_has_no_deadline_no_wake_and_survives_400_days() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    let messages = h.take(a);
    assert_eq!(expires_at(&messages), Some(0));
    h.send(a, &ClientMessage::Append { record: update(client(1), 1) });
    h.close(a);
    h.advance(2 * SECOND);
    assert_eq!(h.wake_at, None, "no alarm is set");
    h.advance(400 * 24 * HOUR);
    assert!(!h.deleted);
    let restarted = Harness::on(
        drafftink_relay::RelayConfig { retention: Retention::Never, ..demo() },
        h.store.clone(),
        h.blobs.clone(),
    );
    let mut restarted = restarted;
    let b = restarted.join(client(2), JoinMode::Open, 0);
    assert_eq!(catchup_seqs(&restarted.take(b)), [1]);
}

#[test]
fn the_ttl_is_configuration() {
    let mut cfg = demo();
    cfg.retention = Retention::after(Duration::from_secs(60));
    let mut h = Harness::new(cfg);
    let a = h.open(1);
    h.close(a);
    h.advance(59 * SECOND);
    assert!(!h.deleted);
    h.advance(2 * SECOND);
    assert!(h.deleted);
}

#[test]
fn deleting_a_room_removes_its_images_too() {
    let mut h = demo_room();
    let a = h.open(1);
    let name = drafftink_protocol::v2::ImageName::from_bytes([3; 16]);
    let body =
        record(drafftink_protocol::v2::RecordKind::Image, drafftink_protocol::ClientId::from_bytes([0; 16]), 0, 0, 64);
    let (put, _) = h.relay.image_request(
        h.now,
        drafftink_relay::ImageRequest { proof: &proof(), op: drafftink_relay::ImageOp::Put { name, body: &body } },
    );
    assert_eq!(put.status, drafftink_relay::ImageStatus::Created);
    h.close(a);
    h.advance(TTL + 2 * SECOND);
    assert!(h.deleted);
    assert!(h.blobs.dump().is_empty());
    let (get, _) = h.relay.image_request(
        h.now,
        drafftink_relay::ImageRequest { proof: &proof(), op: drafftink_relay::ImageOp::Get { name } },
    );
    assert_eq!(get.status, drafftink_relay::ImageStatus::RoomMissing);
}

#[test]
fn delete_room_is_for_hosts_that_allow_it() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    h.send(a, &ClientMessage::DeleteRoom);
    assert_eq!(error_code(&h.take(a)), Some(ErrorCode::Forbidden));
    let mut cfg = demo();
    cfg.retention = Retention::Never;
    cfg.allow_delete_room = true;
    let mut h = Harness::new(cfg);
    let a = h.open(1);
    let b = h.open(2);
    h.take(a);
    h.send(a, &ClientMessage::Append { record: update(client(1), 1) });
    h.send(b, &ClientMessage::DeleteRoom);
    assert!(h.deleted && h.is_closed(a) && h.is_closed(b));
    assert!(h.store.dump().is_empty());
}

#[test]
fn a_store_that_fails_to_delete_is_tried_again() {
    let mut h = demo_room();
    let a = h.open(1);
    h.close(a);
    h.advance(TTL - SECOND);
    h.store.fail_next(1);
    h.advance(SECOND + 500);
    assert!(!h.deleted, "the first attempt failed");
    h.advance(2 * SECOND);
    assert!(h.deleted);
}
