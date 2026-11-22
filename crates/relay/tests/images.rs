#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The image routes: membership, limits, quotas and overwrite accounting.

mod common;

use common::*;
use drafftink_protocol::v2::{ImageName, MembershipToken, Proof, RecordKind};
use drafftink_protocol::{ClientId, MAX_IMAGE_BYTES, SEAL_OVERHEAD};
use drafftink_relay::{ImageBody, ImageOp, ImageRequest, ImageStatus};

fn name(n: u8) -> ImageName {
    ImageName::from_bytes([n; 16])
}

fn sealed_image(total: usize) -> Vec<u8> {
    record(RecordKind::Image, ClientId::from_bytes([0; 16]), 0, 0, total - SEAL_OVERHEAD)
}

fn put(h: &mut Harness, n: u8, body: &[u8]) -> drafftink_relay::ImageResponse {
    h.relay.image_request(h.now, ImageRequest { proof: &proof(), op: ImageOp::Put { name: name(n), body } }).0
}

fn room_with_a_member() -> Harness {
    let mut h = Harness::self_host();
    h.open(1);
    h
}

#[test]
fn a_member_stores_reads_and_lists_an_image() {
    let mut h = room_with_a_member();
    let body = sealed_image(500);
    let response = put(&mut h, 1, &body);
    assert_eq!(response.status, ImageStatus::Created);
    assert_eq!(response.body, ImageBody::Totals(drafftink_relay::BlobStats { count: 1, bytes: 500 }));
    let got = h.relay.image_request(h.now, ImageRequest { proof: &proof(), op: ImageOp::Get { name: name(1) } }).0;
    assert_eq!((got.status, got.body), (ImageStatus::Ok, ImageBody::Sealed(body)));
    let list = h.relay.image_request(h.now, ImageRequest { proof: &proof(), op: ImageOp::List }).0;
    assert_eq!(list.body, ImageBody::List(vec![drafftink_relay::BlobInfo { name: name(1), size: 500 }]));
}

#[test]
fn a_non_member_gets_401_and_nothing_is_stored() {
    let mut h = room_with_a_member();
    let wrong = Proof::token(&MembershipToken::from_bytes([1; 32]));
    let body = sealed_image(500);
    for op in [ImageOp::Put { name: name(1), body: &body }, ImageOp::Get { name: name(1) }, ImageOp::List] {
        let response = h.relay.image_request(h.now, ImageRequest { proof: &wrong, op }).0;
        assert_eq!(response.status, ImageStatus::Unauthorized);
    }
    assert!(h.blobs.dump().is_empty());
}

#[test]
fn a_room_that_does_not_exist_answers_404_and_is_not_created() {
    let mut h = Harness::self_host();
    let body = sealed_image(500);
    assert_eq!(put(&mut h, 1, &body).status, ImageStatus::RoomMissing);
    assert!(h.store.dump().is_empty());
    let join = h.join(client(1), drafftink_protocol::v2::JoinMode::Open, 0);
    assert_eq!(error_code(&h.take(join)), Some(drafftink_protocol::v2::ErrorCode::RoomMissing));
}

#[test]
fn an_unknown_image_is_404() {
    let mut h = room_with_a_member();
    let got = h.relay.image_request(h.now, ImageRequest { proof: &proof(), op: ImageOp::Get { name: name(7) } }).0;
    assert_eq!(got.status, ImageStatus::ImageMissing);
}

#[test]
fn the_size_limit_is_exact() {
    let mut h = room_with_a_member();
    assert_eq!(put(&mut h, 1, &sealed_image(MAX_IMAGE_BYTES + SEAL_OVERHEAD)).status, ImageStatus::Created);
    assert_eq!(put(&mut h, 2, &sealed_image(MAX_IMAGE_BYTES + SEAL_OVERHEAD + 1)).status, ImageStatus::TooLarge);
}

#[test]
fn a_body_that_is_not_a_sealed_image_is_a_bad_request() {
    let mut h = room_with_a_member();
    assert_eq!(put(&mut h, 1, b"not a record").status, ImageStatus::BadRequest);
    assert_eq!(put(&mut h, 2, &update(client(1), 1)).status, ImageStatus::BadRequest, "wrong kind");
}

#[test]
fn the_room_total_and_the_image_count_are_enforced() {
    let mut cfg = demo();
    cfg.retention = drafftink_relay::Retention::Never;
    cfg.limits.max_room_image_bytes = 2_000;
    cfg.limits.max_room_images = 3;
    cfg.max_image_puts_per_sec = 100;
    let mut h = Harness::new(cfg);
    h.open(1);
    assert_eq!(put(&mut h, 1, &sealed_image(1000)).status, ImageStatus::Created);
    assert_eq!(put(&mut h, 2, &sealed_image(1000)).status, ImageStatus::Created, "exactly at the limit");
    assert_eq!(put(&mut h, 3, &sealed_image(SEAL_OVERHEAD)).status, ImageStatus::RoomFull, "one byte over");
    assert_eq!(put(&mut h, 1, &sealed_image(900)).status, ImageStatus::Created, "an overwrite is the difference");
    assert_eq!(put(&mut h, 4, &sealed_image(100)).status, ImageStatus::Created);
    assert_eq!(put(&mut h, 5, &sealed_image(SEAL_OVERHEAD)).status, ImageStatus::RoomFull, "the count: three images");
}

#[test]
fn uploads_are_rate_limited_with_a_wait() {
    let mut h = room_with_a_member();
    let body = sealed_image(200);
    let responses: Vec<_> = (0..40u8).map(|n| put(&mut h, n, &body)).collect();
    let limited = responses.iter().find(|r| r.status == ImageStatus::RateLimited).expect("a limit");
    assert!(limited.retry_after_ms.is_some());
    assert_eq!(ImageStatus::RateLimited.http(), 429);
}

#[test]
fn a_failing_store_is_a_503_and_leaves_no_trace() {
    let mut h = room_with_a_member();
    h.blobs.fail_next(1);
    assert_eq!(put(&mut h, 1, &sealed_image(300)).status, ImageStatus::Unavailable);
    assert_eq!(put(&mut h, 1, &sealed_image(300)).status, ImageStatus::Created);
}

#[test]
fn statuses_carry_the_http_codes_and_words_of_the_design() {
    let table = [
        (ImageStatus::Created, 201, "created"),
        (ImageStatus::Ok, 200, "ok"),
        (ImageStatus::Unauthorized, 401, "unauthorized"),
        (ImageStatus::ImageMissing, 404, "image_missing"),
        (ImageStatus::TooLarge, 413, "image_too_large"),
        (ImageStatus::RoomFull, 507, "room_images_full"),
        (ImageStatus::RateLimited, 429, "rate_limited"),
        (ImageStatus::Unavailable, 503, "unavailable"),
    ];
    for (status, http, code) in table {
        assert_eq!((status.http(), status.code()), (http, code));
    }
}
