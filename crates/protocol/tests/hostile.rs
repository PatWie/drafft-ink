#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Hostile input never panics and never allocates beyond the input.
mod common;
use common::*;
use drafftink_protocol::*;

fn all_frames() -> Vec<(bool, Vec<u8>)> {
    let mut out = Vec::new();
    for m in client_samples() {
        out.push((true, m.encode().unwrap()));
    }
    for m in server_samples() {
        out.push((false, m.encode().unwrap()));
    }
    out
}

/// Decodes as both directions; when it decodes, the result must re-encode and decode equal.
fn probe(bytes: &[u8]) {
    if let Ok(m) = ClientMessage::decode(bytes) {
        let again = m.encode().unwrap();
        assert_eq!(ClientMessage::decode(&again).unwrap(), m);
    }
    if let Ok(m) = ServerMessage::decode(bytes) {
        let again = m.encode().unwrap();
        assert_eq!(ServerMessage::decode(&again).unwrap(), m);
    }
}

#[test]
fn every_truncation_of_every_frame_is_an_error() {
    for (is_client, frame) in all_frames() {
        for cut in 0..frame.len() {
            let part = &frame[..cut];
            if is_client {
                assert!(ClientMessage::decode(part).is_err(), "client cut at {cut}");
            } else {
                assert!(ServerMessage::decode(part).is_err(), "server cut at {cut}");
            }
            probe(part);
        }
    }
}

#[test]
fn trailing_bytes_are_an_error() {
    for (is_client, mut frame) in all_frames() {
        frame.push(0);
        if is_client {
            assert!(matches!(
                ClientMessage::decode(&frame),
                Err(DecodeError::TrailingBytes(1))
            ));
        } else {
            assert!(matches!(
                ServerMessage::decode(&frame),
                Err(DecodeError::TrailingBytes(1))
            ));
        }
    }
}

#[test]
fn empty_frame() {
    assert_eq!(ClientMessage::decode(&[]), Err(DecodeError::Empty));
    assert_eq!(ServerMessage::decode(&[]), Err(DecodeError::Empty));
}

#[test]
fn every_single_bit_flip_is_handled() {
    for (_, frame) in all_frames() {
        for i in 0..frame.len().min(200) {
            for bit in 0..8 {
                let mut f = frame.clone();
                f[i] ^= 1 << bit;
                probe(&f);
            }
        }
    }
}

#[test]
fn random_mutations_and_random_bytes_are_handled() {
    let frames = all_frames();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..30_000 {
        let (_, base) = &frames[rng.below(frames.len())];
        let mut f = base.clone();
        for _ in 0..1 + rng.below(4) {
            match rng.below(4) {
                0 if !f.is_empty() => {
                    let i = rng.below(f.len());
                    f[i] = rng.next() as u8;
                }
                1 => {
                    let i = rng.below(f.len() + 1);
                    f.insert(i, rng.next() as u8);
                }
                2 if !f.is_empty() => {
                    let i = rng.below(f.len());
                    f.remove(i);
                }
                _ => {
                    let n = rng.below(8);
                    f.extend(rng.bytes(n));
                }
            }
        }
        probe(&f);
    }
    for _ in 0..50_000 {
        let n = rng.below(64);
        let mut f = rng.bytes(n);
        // Bias the tag so most inputs reach a real decoder.
        if let Some(first) = f.first_mut() {
            *first = [0x01, 0x02, 0x03, 0x04, 0x81, 0x82, 0x83, 0x85, 0x87, 0x88][rng.below(10)];
        }
        probe(&f);
    }
}

#[test]
fn huge_declared_lengths_do_not_allocate_or_panic() {
    // varint 0xFFFFFFFF
    let huge = [0xff, 0xff, 0xff, 0xff, 0x0f];
    for tag in [0x02u8, 0x03, 0x82, 0x83, 0x88] {
        let mut f = vec![tag];
        f.extend_from_slice(&[0, 0, 0, 0]); // pad for seq / from fields
        f.extend_from_slice(&huge);
        probe(&f);
        let mut g = vec![tag];
        g.extend_from_slice(&huge);
        probe(&g);
    }
    // A Joined that claims 256 peers with no bytes behind them.
    let mut f = vec![0x81, 1, 0, 0]; // version, empty server string
    f.push(1);
    f.push(b'r');
    f.extend_from_slice(&[0; 16]);
    f.extend_from_slice(&[0x80, 0x02]); // 256 peers
    assert!(matches!(
        ServerMessage::decode(&f),
        Err(DecodeError::Truncated(_))
    ));
    // An awareness that claims 512 ids with no bytes behind them.
    let mut f = vec![0x04, 8, 0x80, 0x04];
    assert!(matches!(
        ClientMessage::decode(&f),
        Err(DecodeError::Truncated(_))
    ));
    f[2] = 0xff; // now above the limit
    f.truncate(3);
    f.extend_from_slice(&[0xff, 0xff, 0x0f]);
    assert!(ClientMessage::decode(&f).is_err());
}

#[test]
fn overlong_and_overflowing_varints_are_rejected() {
    // Six continuation bytes.
    let f = [0x03, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00];
    assert!(matches!(
        ClientMessage::decode(&f),
        Err(DecodeError::BadVarint(_))
    ));
    // Fifth byte carries bits above u32.
    let f = [0x03, 0xff, 0xff, 0xff, 0xff, 0x1f];
    assert!(matches!(
        ClientMessage::decode(&f),
        Err(DecodeError::BadVarint(_))
    ));
}

#[test]
fn oversize_frames_are_refused_before_parsing() {
    let f = vec![0x02u8; MAX_FRAME_BYTES + 1];
    assert!(matches!(
        ClientMessage::decode(&f),
        Err(DecodeError::FrameTooLarge { .. })
    ));
    assert!(matches!(
        ServerMessage::decode(&f),
        Err(DecodeError::FrameTooLarge { .. })
    ));
    // An update above its field limit but inside the frame limit.
    let mut f = ClientMessage::Update {
        seq: 1,
        update: vec![1; 10],
    }
    .encode()
    .unwrap();
    f.truncate(5);
    f.extend_from_slice(&[0x80, 0x80, 0x80, 0x03]); // length 6_291_456 > 4 MiB
    f.extend(vec![0u8; MAX_UPDATE_BYTES + 10]);
    assert!(matches!(
        ClientMessage::decode(&f),
        Err(DecodeError::FieldTooLong { .. })
    ));
}

#[test]
fn invalid_text_and_values_are_rejected() {
    // Bad UTF-8 in a room id.
    let mut f = vec![0x01, 1, 0, 1, 0, 2, 0xc3, 0x28];
    f.extend_from_slice(&[0; 16]);
    f.extend_from_slice(&[0, 0, 0]);
    assert!(matches!(
        ClientMessage::decode(&f),
        Err(DecodeError::InvalidUtf8(_))
    ));
    // Room id with a space.
    let mut f = vec![0x01, 1, 0, 1, 0, 1, b' '];
    f.extend_from_slice(&[0; 16]);
    f.extend_from_slice(&[0, 0, 0]);
    assert!(matches!(
        ClientMessage::decode(&f),
        Err(DecodeError::Invalid(_))
    ));
    // Bool byte 2.
    let mut f = vec![0x01, 1, 0, 1, 0, 1, b'r'];
    f.extend_from_slice(&[0; 16]);
    f.extend_from_slice(&[2, 0, 0]);
    assert!(matches!(
        ClientMessage::decode(&f),
        Err(DecodeError::UnknownValue { .. })
    ));
    // Unknown awareness flag bit.
    assert!(matches!(
        ClientMessage::decode(&[0x04, 0x10]),
        Err(DecodeError::UnknownValue { .. })
    ));
    // Non-finite cursor.
    let mut f = vec![0x04, 2];
    f.extend_from_slice(&f64::NAN.to_le_bytes());
    f.extend_from_slice(&0f64.to_le_bytes());
    assert!(matches!(
        ClientMessage::decode(&f),
        Err(DecodeError::Invalid(_))
    ));
    // Colour above 24 bits.
    let mut f = vec![0x04, 1, 0];
    f.extend_from_slice(&0x0100_0000u32.to_le_bytes());
    assert!(matches!(
        ClientMessage::decode(&f),
        Err(DecodeError::Invalid(_))
    ));
    // Unknown error code and sync kind.
    assert!(matches!(
        ServerMessage::decode(&[0x88, 99, 0, 0]),
        Err(DecodeError::UnknownValue { .. })
    ));
    assert!(matches!(
        ServerMessage::decode(&[0x82, 7, 0, 0]),
        Err(DecodeError::UnknownValue { .. })
    ));
    // Empty update.
    assert!(matches!(
        ClientMessage::decode(&[0x02, 0, 0, 0, 0, 0]),
        Err(DecodeError::Invalid(_))
    ));
}

#[test]
fn hostile_json_never_panics() {
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let deep_obj = format!("{}1{}", "{\"a\":".repeat(50_000), "}".repeat(50_000));
    let cases = [
        "",
        "null",
        "[]",
        "{}",
        "{\"type\":\"nope\"}",
        "{\"type\":\"update\"}",
        "{\"type\":\"update\",\"seq\":1,\"update\":\"zz\"}",
        "{\"type\":\"update\",\"seq\":1,\"update\":\"abc\"}",
        "{\"type\":\"update\",\"seq\":-1,\"update\":\"00\"}",
        "{\"type\":\"update\",\"seq\":1,\"update\":\"\"}",
        "{\"type\":\"ping\",\"nonce\":1e99}",
        "{\"type\":\"awareness\",\"cursor\":{\"x\":1e999,\"y\":0}}",
        "{\"type\":\"error\",\"code\":\"bogus\",\"message\":\"\"}",
        "\u{0}\u{1}\u{2}",
        "{\"type\":\"leave\"",
        &deep,
        &deep_obj,
    ];
    for c in cases {
        let _ = ClientMessage::from_json(c);
        let _ = ServerMessage::from_json(c);
    }
    let too_big = " ".repeat(MAX_JSON_BYTES + 1);
    assert!(matches!(
        ClientMessage::from_json(&too_big),
        Err(DecodeError::FrameTooLarge { .. })
    ));
    // Valid JSON that breaks a rule is refused by validation.
    let long_name = format!(
        "{{\"type\":\"awareness\",\"user\":{{\"name\":\"{}\",\"color\":1}}}}",
        "x".repeat(200)
    );
    assert!(matches!(
        ClientMessage::from_json(&long_name),
        Err(DecodeError::Invalid(_))
    ));
    let bad_color = "{\"type\":\"awareness\",\"user\":{\"name\":\"a\",\"color\":16777216}}";
    assert!(matches!(
        ClientMessage::from_json(bad_color),
        Err(DecodeError::Invalid(_))
    ));
}
