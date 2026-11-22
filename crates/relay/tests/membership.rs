#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Membership against the room id, `open` and `create`, and the first message.

mod common;

use common::*;
use drafftink_protocol::ClientId;
use drafftink_protocol::v2::{
    ClientMessage, ErrorCode, JoinMode, MembershipToken, Proof, ProofScheme, RoomId, RoomState, ServerMessage,
};

fn join_with(h: &mut Harness, proof: Proof, room: RoomId, mode: JoinMode) -> Vec<ServerMessage> {
    let conn = h.connect();
    let message = ClientMessage::Join {
        min_version: 2,
        max_version: 2,
        room,
        client_id: client(1),
        mode,
        proof,
        last_seq: 0,
        captcha: None,
    };
    h.send(conn, &message);
    assert!(h.relay.connection_count() <= 1);
    h.take(conn)
}

#[test]
fn the_right_token_joins() {
    let mut h = Harness::self_host();
    let conn = h.join(client(1), JoinMode::Create, 0);
    let messages = h.take(conn);
    assert!(
        matches!(&messages[0], ServerMessage::Joined { room_state: RoomState::Created, head_seq: 0, .. }),
        "{messages:?}"
    );
    assert!(matches!(&messages[1], ServerMessage::Catchup { snapshot: None, more: false, .. }));
    assert!(!h.is_closed(conn));
}

#[test]
fn a_wrong_token_a_token_for_another_room_and_a_short_token_are_unauthorized() {
    let wrong = Proof::token(&MembershipToken::from_bytes([1; 32]));
    let short = Proof::new(ProofScheme::Token, vec![9; 31]);
    let long = Proof::new(ProofScheme::Token, vec![9; 33]);
    let empty = Proof::new(ProofScheme::Token, Vec::new());
    for proof in [wrong, short, long, empty] {
        let mut h = Harness::self_host();
        let messages = join_with(&mut h, proof, room_id(), JoinMode::Create);
        assert_eq!(error_code(&messages), Some(ErrorCode::Unauthorized), "{messages:?}");
        assert!(h.store.dump().is_empty());
        let mut again = Harness::self_host();
        let missing = join_with(&mut again, Proof::token(&token()), room_id(), JoinMode::Open);
        assert_eq!(error_code(&missing), Some(ErrorCode::RoomMissing), "a refused join created nothing");
    }
}

#[test]
fn a_good_token_for_a_room_this_relay_does_not_serve_is_unauthorized() {
    let other = RoomId::from_token(&MembershipToken::from_bytes([5; 32]));
    let mut h = Harness::self_host();
    let messages = join_with(&mut h, Proof::token(&MembershipToken::from_bytes([5; 32])), other, JoinMode::Create);
    assert_eq!(error_code(&messages), Some(ErrorCode::Unauthorized));
}

#[test]
fn open_on_a_missing_room_creates_nothing() {
    let mut h = Harness::self_host();
    let conn = h.join(client(1), JoinMode::Open, 0);
    let messages = h.take(conn);
    assert_eq!(error_code(&messages), Some(ErrorCode::RoomMissing), "{messages:?}");
    assert!(h.is_closed(conn));
    assert!(h.ops.is_empty(), "no store write: {:?}", h.ops);
    assert!(h.store.dump().is_empty());
    let again = h.join(client(1), JoinMode::Open, 0);
    assert_eq!(error_code(&h.take(again)), Some(ErrorCode::RoomMissing));
}

#[test]
fn create_on_an_existing_room_is_an_open() {
    let mut h = Harness::self_host();
    h.open(1);
    let second = h.join(client(2), JoinMode::Create, 0);
    let messages = h.take(second);
    assert!(matches!(&messages[0], ServerMessage::Joined { room_state: RoomState::Existing, .. }), "{messages:?}");
}

#[test]
fn open_on_an_existing_room_joins() {
    let mut h = Harness::self_host();
    h.open(1);
    let second = h.join(client(2), JoinMode::Open, 0);
    assert!(matches!(h.take(second)[0], ServerMessage::Joined { room_state: RoomState::Existing, .. }));
}

#[test]
fn the_first_message_must_be_a_join() {
    let mut h = Harness::self_host();
    let conn = h.connect();
    h.send(conn, &ClientMessage::Catchup { after_seq: 0 });
    assert_eq!(error_code(&h.take(conn)), Some(ErrorCode::NotJoined));
    assert!(h.is_closed(conn));
}

#[test]
fn a_second_join_on_a_joined_connection_is_refused() {
    let mut h = Harness::self_host();
    let conn = h.open(1);
    h.take(conn);
    h.send(conn, &join_message(client(1), JoinMode::Open, 0));
    assert_eq!(error_code(&h.take(conn)), Some(ErrorCode::AlreadyJoined));
    assert!(h.is_closed(conn));
}

#[test]
fn a_connection_that_never_joins_is_closed_after_the_join_timeout() {
    let mut h = Harness::self_host();
    let conn = h.connect();
    h.advance(h.relay.config().join_timeout_ms + 1);
    assert_eq!(error_code(&h.take(conn)), Some(ErrorCode::NotJoined));
    assert!(h.is_closed(conn));
    assert_eq!(h.relay.connection_count(), 0);
}

#[test]
fn a_version_one_join_gets_a_plain_refusal() {
    let mut h = Harness::self_host();
    let conn = h.connect();
    let v1 = drafftink_protocol::ClientMessage::Join {
        min_version: 1,
        max_version: 1,
        room: drafftink_protocol::RoomId::new("plain-room").expect("room"),
        client_id: ClientId::from_bytes([1; 16]),
        auth_token: None,
        version: Vec::new(),
        user: None,
    };
    h.send_raw(conn, &v1.encode().expect("encodes"));
    let messages = h.take(conn);
    let Some(ServerMessage::Error { code: ErrorCode::UnsupportedVersion, message, .. }) = messages.first() else {
        panic!("{messages:?}")
    };
    assert!(message.contains("end-to-end encrypted"), "{message}");
}

#[test]
fn the_same_client_joining_again_replaces_the_old_connection() {
    let mut h = Harness::self_host();
    let first = h.open(1);
    let other = h.open(2);
    h.take(first);
    h.take(other);
    let second = h.join(client(1), JoinMode::Open, 0);
    assert_eq!(error_code(&h.take(first)), Some(ErrorCode::Replaced));
    assert!(h.is_closed(first));
    assert!(matches!(h.take(second)[0], ServerMessage::Joined { .. }));
    assert_eq!(h.relay.member_count(), 2);
}

#[test]
fn a_full_room_refuses_one_more_person_but_not_a_returning_one() {
    let mut cfg = demo();
    cfg.max_peers = 2;
    let mut h = Harness::new(cfg);
    h.open(1);
    h.open(2);
    let third = h.join(client(3), JoinMode::Create, 0);
    assert_eq!(error_code(&h.take(third)), Some(ErrorCode::RoomFull));
    let again = h.join(client(2), JoinMode::Open, 0);
    assert!(matches!(h.take(again)[0], ServerMessage::Joined { .. }));
}

#[test]
fn peers_hear_about_joins_and_leaves_by_client_id_only() {
    let mut h = Harness::self_host();
    let a = h.open(1);
    h.take(a);
    let b = h.open(2);
    let joined = h.take(b);
    assert!(matches!(&joined[0], ServerMessage::Joined { peers, .. } if peers == &[client(1)]), "{joined:?}");
    assert!(matches!(h.take(a)[0], ServerMessage::PeerJoined { client_id, .. } if client_id == client(2)));
    h.close(b);
    assert!(matches!(h.take(a)[0], ServerMessage::PeerLeft { client_id } if client_id == client(2)));
}

#[test]
fn keepalive_text_is_answered_and_other_text_is_refused() {
    let mut h = Harness::self_host();
    let conn = h.open(1);
    h.take(conn);
    let out = h.relay.on_text(conn, h.now, "ping");
    assert_eq!(out.effects, vec![drafftink_relay::Effect::Text { conn, text: "pong" }]);
    let out = h.relay.on_text(conn, h.now, "hello");
    assert!(out.effects.iter().any(|e| matches!(e, drafftink_relay::Effect::Close { .. })));
}
