#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;
use common::*;
use drafftink_protocol::*;

fn join(min: u16, max: u16) -> ClientMessage {
    ClientMessage::Join {
        min_version: min,
        max_version: max,
        room: RoomId::new("room").unwrap(),
        client_id: cid(1),
        auth_token: None,
        version: vec![],
        user: None,
    }
}

#[test]
fn negotiation_picks_the_highest_common_version() {
    assert_eq!(negotiate_version(1, 1), Some(PROTOCOL_VERSION));
    assert_eq!(
        negotiate_version(MIN_PROTOCOL_VERSION, PROTOCOL_VERSION + 5),
        Some(PROTOCOL_VERSION)
    );
    assert_eq!(
        negotiate_version(PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 2),
        None
    );
    assert_eq!(negotiate_version(0, 0), None, "version 0 is not spoken");
    assert_eq!(negotiate_version(5, 3), None);
}

#[test]
fn a_future_only_client_gets_unsupported_version() {
    let frame = join(PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 3)
        .encode()
        .unwrap();
    let err = ClientMessage::decode(&frame).unwrap_err();
    assert_eq!(
        err,
        DecodeError::UnsupportedVersion {
            client_min: PROTOCOL_VERSION + 1,
            client_max: PROTOCOL_VERSION + 3,
            ours_min: MIN_PROTOCOL_VERSION,
            ours_max: PROTOCOL_VERSION,
        }
    );
    let reply = ServerMessage::error_for_decode(&err);
    match &reply {
        ServerMessage::Error { code, message, .. } => {
            assert_eq!(*code, ErrorCode::UnsupportedVersion);
            assert!(message.contains("client speaks"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    // The reply itself is a valid frame.
    assert_eq!(
        ServerMessage::decode(&reply.encode().unwrap()).unwrap(),
        reply
    );
}

#[test]
fn version_check_happens_before_the_rest_of_the_frame_is_read() {
    // A future client may lay out the rest of the Join differently. Only the first five
    // bytes (tag, min, max) are promised to stay put.
    let mut frame = vec![0x01];
    frame.extend_from_slice(&(PROTOCOL_VERSION + 1).to_le_bytes());
    frame.extend_from_slice(&(PROTOCOL_VERSION + 9).to_le_bytes());
    frame.extend_from_slice(b"\xff\xff\xff garbage in a layout we do not know");
    assert!(matches!(
        ClientMessage::decode(&frame),
        Err(DecodeError::UnsupportedVersion { .. })
    ));
}

#[test]
fn a_client_range_that_includes_our_version_is_accepted() {
    let frame = join(MIN_PROTOCOL_VERSION, PROTOCOL_VERSION + 4)
        .encode()
        .unwrap();
    assert!(ClientMessage::decode(&frame).is_ok());
}

#[test]
fn json_join_is_version_checked_too() {
    let text = join(PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 1)
        .to_json()
        .unwrap();
    assert!(matches!(
        ClientMessage::from_json(&text),
        Err(DecodeError::UnsupportedVersion { .. })
    ));
}

#[test]
fn inverted_range_is_invalid() {
    assert!(join(3, 2).encode().is_err());
}

#[test]
fn unknown_tags_are_reported_not_ignored() {
    assert_eq!(
        ClientMessage::decode(&[0x7f]),
        Err(DecodeError::UnknownTag(0x7f))
    );
    assert_eq!(
        ServerMessage::decode(&[0x01]),
        Err(DecodeError::UnknownTag(0x01))
    );
    // Client and server tag spaces do not overlap, so a mixed-up direction is an error.
    let c = ClientMessage::Leave.encode().unwrap();
    assert!(ServerMessage::decode(&c).is_err());
    let s = ServerMessage::Ack { seq: 1 }.encode().unwrap();
    assert!(ClientMessage::decode(&s).is_err());
}
