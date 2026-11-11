#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;
use common::*;
use drafftink_protocol::*;

#[test]
fn client_messages_round_trip_binary() {
    for msg in client_samples() {
        let frame = msg.encode().unwrap();
        assert_eq!(ClientMessage::decode(&frame).unwrap(), msg, "{msg:?}");
        // Encoding is canonical: decode then encode gives the same bytes.
        assert_eq!(ClientMessage::decode(&frame).unwrap().encode().unwrap(), frame);
    }
}

#[test]
fn server_messages_round_trip_binary() {
    for msg in server_samples() {
        let frame = msg.encode().unwrap();
        assert_eq!(ServerMessage::decode(&frame).unwrap(), msg, "{msg:?}");
        assert_eq!(ServerMessage::decode(&frame).unwrap().encode().unwrap(), frame);
    }
}

#[test]
fn client_messages_round_trip_json() {
    for msg in client_samples() {
        let text = msg.to_json().unwrap();
        assert_eq!(ClientMessage::from_json(&text).unwrap(), msg, "{text}");
    }
}

#[test]
fn server_messages_round_trip_json() {
    for msg in server_samples() {
        let text = msg.to_json().unwrap();
        assert_eq!(ServerMessage::from_json(&text).unwrap(), msg, "{text}");
    }
}

#[test]
fn json_form_is_readable() {
    let text = ClientMessage::Update { seq: 3, update: vec![0xde, 0xad] }.to_json().unwrap();
    assert_eq!(text, r#"{"type":"update","seq":3,"update":"dead"}"#);
    let text = ClientMessage::Leave.to_json().unwrap();
    assert_eq!(text, r#"{"type":"leave"}"#);
}

#[test]
fn binary_payloads_are_raw_not_base64() {
    // A 1000 byte update costs 1000 bytes plus a small envelope (tag, seq, 2 byte length).
    let payload = vec![0xabu8; 1000];
    let frame = ClientMessage::Update { seq: 1, update: payload.clone() }.encode().unwrap();
    assert_eq!(frame.len(), 1000 + 1 + 4 + 2);
    assert!(frame.ends_with(&payload));
    // The old scheme cost 4/3 of that in base64 alone.
    let json = ClientMessage::Update { seq: 1, update: payload }.to_json().unwrap();
    assert!(json.len() > 2 * 1000);
}

#[test]
fn large_payload_near_the_limit_round_trips() {
    let update = vec![7u8; MAX_UPDATE_BYTES];
    let frame = ServerMessage::Update { from: cid(1), update: update.clone() }.encode().unwrap();
    match ServerMessage::decode(&frame).unwrap() {
        ServerMessage::Update { update: got, .. } => assert_eq!(got, update),
        other => panic!("{other:?}"),
    }
}

#[test]
fn oversize_messages_are_refused_when_encoding() {
    let big = ClientMessage::Update { seq: 0, update: vec![0; MAX_UPDATE_BYTES + 1] };
    assert!(matches!(big.encode(), Err(EncodeError::Invalid(_))));
    assert!(matches!(big.to_json(), Err(EncodeError::Invalid(_))));

    let empty = ClientMessage::Update { seq: 0, update: vec![] };
    assert!(matches!(empty.encode(), Err(EncodeError::Invalid(_))));

    let mut a = full_awareness();
    a.selection = (0..=MAX_SELECTION_IDS).map(|i| i.to_string()).collect();
    assert!(ClientMessage::Awareness(a).encode().is_err());

    let mut a = full_awareness();
    a.user = Some(UserInfo { name: "x".repeat(MAX_USER_NAME_BYTES + 1), color: 0 });
    assert!(ClientMessage::Awareness(a).encode().is_err());

    let mut a = full_awareness();
    a.cursor = Some(Point { x: f64::NAN, y: 0.0 });
    assert!(ClientMessage::Awareness(a).encode().is_err());

    let sync = ServerMessage::SyncResponse {
        kind: SyncKind::Snapshot,
        data: vec![0; MAX_SYNC_BYTES + 1],
        server_version: vec![],
    };
    assert!(sync.encode().is_err());
}

#[test]
fn error_message_builder_cuts_on_a_char_boundary() {
    let long = "é".repeat(MAX_ERROR_MESSAGE_BYTES);
    let msg = ServerMessage::error(ErrorCode::Malformed, long);
    let frame = msg.encode().unwrap();
    assert_eq!(ServerMessage::decode(&frame).unwrap(), msg);
}

#[test]
fn room_ids_are_validated() {
    let reason = |r: Result<RoomId, ValidationError>| r.unwrap_err().reason;
    assert_eq!(reason(RoomId::new("")), Rule::Empty);
    assert_eq!(reason(RoomId::new("a b")), Rule::BadCharacter);
    assert_eq!(reason(RoomId::new("a/b")), Rule::BadCharacter);
    assert_eq!(reason(RoomId::new("é")), Rule::BadCharacter);
    assert_eq!(reason(RoomId::new("x".repeat(MAX_ROOM_ID_BYTES + 1))), Rule::TooLong);
    assert!(RoomId::new("x".repeat(MAX_ROOM_ID_BYTES)).is_ok());
    let id = RoomId::from_random_bytes([0xab; 16]);
    assert_eq!(id.as_str(), "abababababababababababababababab");
}

#[test]
fn client_id_hex_round_trip() {
    let id = ClientId::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 255]);
    assert_eq!(ClientId::from_hex(&id.to_string()).unwrap(), id);
    assert!(ClientId::from_hex("zz").is_err());
    assert!(ClientId::from_hex(&"0".repeat(31)).is_err());
}

#[test]
fn error_codes_have_stable_numbers() {
    for n in 0..=255u8 {
        if let Some(code) = ErrorCode::from_u8(n) {
            assert_eq!(code.as_u8(), n);
        }
    }
    assert_eq!(ErrorCode::from_u8(1), Some(ErrorCode::Malformed));
    assert_eq!(ErrorCode::from_u8(14), Some(ErrorCode::Internal));
    assert_eq!(ErrorCode::from_u8(0), None);
    assert_eq!(ErrorCode::from_u8(15), None);
    assert!(ErrorCode::Unauthorized.closes_connection());
    assert!(!ErrorCode::ResyncRequired.closes_connection());
    assert!(!ErrorCode::Unauthorized.retryable());
    assert!(ErrorCode::Unavailable.retryable());
}

#[test]
fn reconnect_delay_grows_then_caps() {
    for jitter in [0u32, 1, 12345, u32::MAX] {
        let mut prev_floor = 0;
        for attempt in 0..40 {
            let d = reconnect_delay_ms(attempt, jitter);
            assert!(d <= RECONNECT_MAX_MS, "attempt {attempt}: {d}");
            let full = RECONNECT_BASE_MS.saturating_mul(1 << attempt.min(16)).min(RECONNECT_MAX_MS);
            assert!(d >= full / 2 && d <= full);
            assert!(full / 2 >= prev_floor);
            prev_floor = full / 2;
        }
    }
    assert!(reconnect_delay_ms(0, 0) >= RECONNECT_BASE_MS / 2);
    assert_eq!(reconnect_delay_ms(100, 0), RECONNECT_MAX_MS / 2);
}
