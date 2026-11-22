#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Protocol v2 (sealed rooms): round trips, strict decoding, hostile frames, the `Join` handshake and the limits.
//! The sealed records here are made by hand (header, nonce, filler): the protocol never opens one, and the cipher
//! belongs to `drafftink-seal`.
mod common;
use common::{Rng, cid};
use drafftink_protocol::v2::{
    CatchupSnapshot, ClientMessage as C2, ErrorCode as E2, ImageName, JoinMode, MembershipToken, PresenceEntry, Proof,
    ProofScheme, RecordError, RecordHeader, RecordKind, RoomId, RoomLimits, RoomState, ServerMessage as S2,
    StoredRecord,
};
use drafftink_protocol::*;

/// A sealed record that has the right shape and no meaning: the header, a nonce, `body` bytes and a tag.
fn record(kind: RecordKind, sender: ClientId, ctr: u64, covers: u64, body: usize) -> Vec<u8> {
    let header = RecordHeader::new(kind, sender, ctr, covers).unwrap();
    let filler = (0..RECORD_NONCE_BYTES + RECORD_LEN_PREFIX_BYTES + body + RECORD_TAG_BYTES).map(|i| (i * 31) as u8);
    header.to_bytes().into_iter().chain(filler).collect()
}

fn update(sender: u8, ctr: u64, body: usize) -> Vec<u8> {
    record(RecordKind::Update, cid(sender), ctr, 0, body)
}

fn token() -> MembershipToken {
    MembershipToken::from_bytes(std::array::from_fn(|i| 0xc0 ^ i as u8))
}

fn room() -> RoomId {
    RoomId::from_token(&token())
}

fn stored(seq: u64, sender: u8, ctr: u64) -> StoredRecord {
    StoredRecord { seq, from: cid(sender), ctr, record: update(sender, ctr, 10) }
}

/// The fields of a v2 `Join`, so that a test can change one with `..J::default()`.
struct J {
    min: u16,
    max: u16,
    mode: JoinMode,
    proof: Proof,
    last_seq: u64,
    captcha: Option<String>,
}

impl Default for J {
    fn default() -> Self {
        J {
            min: 2,
            max: 2,
            mode: JoinMode::Create,
            proof: Proof::token(&token()),
            last_seq: 99,
            captcha: Some("turnstile".into()),
        }
    }
}

impl J {
    fn msg(self) -> C2 {
        C2::Join {
            min_version: self.min,
            max_version: self.max,
            room: room(),
            client_id: cid(1),
            mode: self.mode,
            proof: self.proof,
            last_seq: self.last_seq,
            captcha: self.captcha,
        }
    }
}

fn join(min: u16, max: u16) -> C2 {
    J { min, max, ..J::default() }.msg()
}

fn client_samples() -> Vec<C2> {
    vec![
        join(2, 2),
        J { mode: JoinMode::Open, captcha: None, last_seq: 0, ..J::default() }.msg(),
        join(1, 7),
        C2::Append { record: update(1, 1, 0) },
        C2::Append { record: update(1, u64::MAX, 5000) },
        C2::PutSnapshot { expected_snapshot_seq: 0, record: record(RecordKind::Snapshot, cid(2), 3, 0, 1000) },
        C2::PutSnapshot { expected_snapshot_seq: 8, record: record(RecordKind::Snapshot, cid(2), 4, u64::MAX, 7) },
        C2::Presence { record: record(RecordKind::Presence, cid(3), 9, 0, 120) },
        C2::Catchup { after_seq: 0 },
        C2::Catchup { after_seq: u64::MAX },
        C2::Direct { to: None, record: record(RecordKind::Direct, cid(4), 1, 0, 33) },
        C2::Direct { to: Some(cid(5)), record: record(RecordKind::Direct, cid(4), 2, 0, 0) },
        C2::DeleteRoom,
        C2::Leave,
    ]
}

/// The fields of a v2 `Joined`, so that a test can change one with `..JD::default()`.
struct JD {
    room_state: RoomState,
    head_seq: u64,
    snapshot_seq: u64,
    expires_at_ms: u64,
    peers: Vec<ClientId>,
    limits: RoomLimits,
}

impl Default for JD {
    fn default() -> Self {
        JD {
            room_state: RoomState::Created,
            head_seq: 12,
            snapshot_seq: 7,
            expires_at_ms: 1_700_000_000_000,
            peers: vec![cid(2), cid(3)],
            limits: RoomLimits::DEFAULT,
        }
    }
}

impl JD {
    fn msg(self) -> S2 {
        S2::Joined {
            protocol_version: 2,
            room_state: self.room_state,
            head_seq: self.head_seq,
            snapshot_seq: self.snapshot_seq,
            log_records: 5,
            log_bytes: 4321,
            expires_at_ms: self.expires_at_ms,
            peers: self.peers,
            limits: self.limits,
        }
    }
}

fn server_samples() -> Vec<S2> {
    vec![
        JD::default().msg(),
        JD {
            room_state: RoomState::Existing,
            peers: vec![],
            expires_at_ms: 0,
            snapshot_seq: 0,
            head_seq: 0,
            ..JD::default()
        }
        .msg(),
        S2::Catchup { snapshot: None, records: vec![], head_seq: 0, more: false },
        S2::Catchup {
            snapshot: Some(CatchupSnapshot { covers_seq: 6, record: record(RecordKind::Snapshot, cid(9), 1, 6, 400) }),
            records: vec![stored(7, 1, 1), stored(8, 2, 5), stored(9, 1, 2)],
            head_seq: 9,
            more: true,
        },
        S2::Record(stored(10, 3, 77)),
        S2::Ack { ctr: u64::MAX, seq: 3 },
        S2::PeerJoined { client_id: cid(6), max_presence_per_sec: 13 },
        S2::PeerLeft { client_id: cid(6) },
        S2::PresenceBundle(vec![]),
        S2::PresenceBundle(vec![
            PresenceEntry { from: cid(1), record: record(RecordKind::Presence, cid(1), 4, 0, 50) },
            PresenceEntry { from: cid(2), record: record(RecordKind::Presence, cid(2), 8, 0, 0) },
        ]),
        S2::Error { code: E2::DailyLimit, message: "come back at midnight".into(), retry_after_ms: Some(3_600_000) },
        S2::Error { code: E2::RoomMissing, message: String::new(), retry_after_ms: None },
        S2::SnapshotResult { ok: true, covers_seq: 6, snapshot_seq: 6, head_seq: 9 },
        S2::SnapshotResult { ok: false, covers_seq: 6, snapshot_seq: 8, head_seq: 9 },
        S2::CompactAdvice { head_seq: 500 },
        S2::Direct { from: cid(7), record: record(RecordKind::Direct, cid(7), 2, 0, 64) },
        S2::Retention { expires_at_ms: 1_700_000_000_000, expiring: false },
        S2::Retention { expires_at_ms: 0, expiring: true },
    ]
}

fn all_frames() -> Vec<(bool, Vec<u8>)> {
    let clients = client_samples().into_iter().map(|m| (true, m.encode().unwrap()));
    clients.chain(server_samples().into_iter().map(|m| (false, m.encode().unwrap()))).collect()
}

// ---- round trips ----------------------------------------------------------------------------

#[test]
fn every_message_round_trips() {
    for m in client_samples() {
        let frame = m.encode().unwrap();
        assert_eq!(C2::decode(&frame).unwrap(), m, "{m:?}");
        assert_eq!(m.encode().unwrap(), frame, "encoding is deterministic");
    }
    for m in server_samples() {
        let frame = m.encode().unwrap();
        assert_eq!(S2::decode(&frame).unwrap(), m, "{m:?}");
        assert_eq!(m.encode().unwrap(), frame, "encoding is deterministic");
    }
}

#[test]
fn client_tags_are_below_0x80_and_relay_tags_above() {
    for m in client_samples() {
        assert!(m.encode().unwrap()[0] < 0x80, "{m:?}");
    }
    for m in server_samples() {
        assert!(m.encode().unwrap()[0] >= 0x80, "{m:?}");
    }
    // A frame sent in the wrong direction is an unknown tag.
    for (is_client, frame) in all_frames() {
        if is_client {
            assert!(matches!(S2::decode(&frame), Err(DecodeError::UnknownTag(_))));
        } else {
            assert!(matches!(C2::decode(&frame), Err(DecodeError::UnknownTag(_))));
        }
    }
}

#[test]
fn the_wire_bytes_of_a_small_frame_are_stable() {
    // tag, after_seq u64 LE
    assert_eq!(C2::Catchup { after_seq: 0x0102 }.encode().unwrap(), [0x13, 2, 1, 0, 0, 0, 0, 0, 0]);
    assert_eq!(C2::DeleteRoom.encode().unwrap(), [0x15]);
    assert_eq!(C2::Leave.encode().unwrap(), [0x07]);
    // tag, ctr u64 LE, seq u64 LE
    assert_eq!(S2::Ack { ctr: 1, seq: 2 }.encode().unwrap(), [0x84, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    // tag, expires u64, expiring bool
    assert_eq!(S2::Retention { expires_at_ms: 0, expiring: true }.encode().unwrap(), [0x8c, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
}

#[test]
fn a_join_has_the_documented_layout() {
    let frame = join(2, 2).encode().unwrap();
    assert_eq!(&frame[..5], [0x01, 2, 0, 2, 0], "tag, min, max");
    assert_eq!(&frame[5..21], room().as_bytes(), "room id");
    assert_eq!(&frame[21..37], cid(1).as_bytes(), "client id");
    assert_eq!(&frame[37..39], [1, 1], "mode create, scheme token");
    assert_eq!(frame[39], 32, "proof length");
    assert_eq!(&frame[40..72], token().expose_bytes(), "proof");
    assert_eq!(&frame[72..80], 99u64.to_le_bytes(), "last_seq");
    assert_eq!(&frame[80..], [1, 9, b't', b'u', b'r', b'n', b's', b't', b'i', b'l', b'e'], "captcha");
}

// ---- hostile frames -------------------------------------------------------------------------

/// Decodes as both directions; when it decodes, the result must encode again and decode to the same message.
fn probe(bytes: &[u8]) {
    if let Ok(m) = C2::decode(bytes) {
        assert_eq!(C2::decode(&m.encode().unwrap()).unwrap(), m);
    }
    if let Ok(m) = S2::decode(bytes) {
        assert_eq!(S2::decode(&m.encode().unwrap()).unwrap(), m);
    }
}

#[test]
fn every_truncation_of_every_frame_is_an_error() {
    for (is_client, frame) in all_frames() {
        for cut in 0..frame.len() {
            let part = &frame[..cut];
            if is_client {
                assert!(C2::decode(part).is_err(), "client frame cut at {cut}");
            } else {
                assert!(S2::decode(part).is_err(), "relay frame cut at {cut}");
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
            assert!(matches!(C2::decode(&frame), Err(DecodeError::TrailingBytes(1))));
        } else {
            assert!(matches!(S2::decode(&frame), Err(DecodeError::TrailingBytes(1))));
        }
    }
}

#[test]
fn every_single_bit_flip_is_handled() {
    for (_, frame) in all_frames() {
        for bit in 0..frame.len() * 8 {
            let mut bytes = frame.clone();
            bytes[bit / 8] ^= 1 << (bit % 8);
            probe(&bytes);
        }
    }
}

#[test]
fn random_mutations_and_random_frames_are_handled() {
    let frames = all_frames();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    for _ in 0..30_000 {
        let (_, frame) = &frames[rng.below(frames.len())];
        let mut bytes = frame.clone();
        for _ in 0..1 + rng.below(6) {
            match rng.below(4) {
                0 => {
                    let at = rng.below(bytes.len());
                    bytes[at] = rng.next() as u8;
                }
                1 => bytes.truncate(rng.below(bytes.len() + 1)),
                2 => {
                    let extra = rng.below(8);
                    bytes.extend(rng.bytes(extra));
                }
                _ => {
                    let at = rng.below(bytes.len());
                    bytes.insert(at, rng.next() as u8);
                }
            }
            if bytes.is_empty() {
                break;
            }
        }
        probe(&bytes);
    }
    for _ in 0..50_000 {
        let len = rng.below(200);
        let mut bytes = rng.bytes(len);
        // Make a known tag likely, so that the fields behind it are reached.
        if let Some(tag) = bytes.first_mut() {
            *tag = [0x01, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x07, 0x81, 0x82, 0x83, 0x87, 0x88, 0x8b][rng.below(14)];
        }
        probe(&bytes);
    }
}

#[test]
fn huge_length_prefixes_are_refused_before_anything_is_allocated() {
    let huge = [0xff, 0xff, 0xff, 0xff, 0x0f];
    // Append { record } with a four gigabyte record.
    assert!(matches!(C2::decode(&[&[0x10][..], &huge].concat()), Err(DecodeError::FieldTooLong { .. })));
    // A Catchup that claims four billion records.
    let catchup = [&[0x82, 0][..], &huge].concat();
    assert!(matches!(S2::decode(&catchup), Err(DecodeError::FieldTooLong { .. })));
    // A Joined that claims 256 peers and has none.
    let mut joined = server_samples()[0].encode().unwrap();
    let peers_at = 1 + 2 + 1 + 8 + 8 + 4 + 4 + 8;
    joined.truncate(peers_at);
    joined.extend_from_slice(&[0xff, 0x01]);
    assert!(S2::decode(&joined).is_err());
    // A count that fits the limit but not the input.
    let bundle = [0x87, 0x40];
    assert!(matches!(S2::decode(&bundle), Err(DecodeError::Truncated(_))));
}

#[test]
fn a_frame_over_the_frame_limit_is_refused() {
    assert!(matches!(C2::decode(&vec![0x10; MAX_FRAME_BYTES + 1]), Err(DecodeError::FrameTooLarge { .. })));
    assert!(matches!(S2::decode(&vec![0x83; MAX_FRAME_BYTES + 1]), Err(DecodeError::FrameTooLarge { .. })));
}

#[test]
fn unknown_enum_values_and_bad_booleans_are_refused() {
    let at = |frame: &mut Vec<u8>, index: usize, value: u8| frame[index] = value;
    let join_frame = join(2, 2).encode().unwrap();
    for (index, what) in [(37, "mode"), (38, "scheme")] {
        let mut frame = join_frame.clone();
        at(&mut frame, index, 9);
        assert!(matches!(C2::decode(&frame), Err(DecodeError::UnknownValue { .. })), "{what}");
    }
    let mut frame = join_frame.clone();
    at(&mut frame, 80, 2); // the captcha flag
    assert!(matches!(C2::decode(&frame), Err(DecodeError::UnknownValue { .. })));

    let joined = server_samples()[0].encode().unwrap();
    let mut frame = joined.clone();
    at(&mut frame, 3, 2); // room_state
    assert!(matches!(S2::decode(&frame), Err(DecodeError::UnknownValue { .. })));
    let mut result = S2::SnapshotResult { ok: true, covers_seq: 1, snapshot_seq: 1, head_seq: 1 }.encode().unwrap();
    at(&mut result, 1, 2);
    assert!(matches!(S2::decode(&result), Err(DecodeError::UnknownValue { .. })));
    let mut error = S2::error(E2::Malformed, "x").encode().unwrap();
    at(&mut error, 1, 0);
    assert!(matches!(S2::decode(&error), Err(DecodeError::UnknownValue { .. })));
    at(&mut error, 1, 200);
    assert!(matches!(S2::decode(&error), Err(DecodeError::UnknownValue { .. })));
}

#[test]
fn a_record_that_is_not_a_record_of_this_kind_is_refused() {
    let good = update(1, 1, 10);
    let bad_version = [&[2u8][..], &good[1..]].concat();
    let bad_kind = [&good[..1], &[77u8], &good[2..]].concat();
    let bad_cover = [&good[..26], &[1u8], &good[27..]].concat();
    let short = good[..SEAL_OVERHEAD - 1].to_vec();
    let presence = record(RecordKind::Presence, cid(1), 1, 0, 10);
    let image_with_sender = [&[1u8, 5][..], &[1u8; 16], &good[18..]].concat();
    for (what, record) in [
        ("version 2", bad_version),
        ("unknown kind", bad_kind),
        ("cover on an update", bad_cover),
        ("too short", short),
        ("empty", vec![]),
        ("a presence record in an Append", presence),
        ("an image with a sender", image_with_sender),
    ] {
        assert!(C2::Append { record: record.clone() }.encode().is_err(), "encode: {what}");
        let frame = [&[0x10u8][..], &[record.len() as u8], &record].concat();
        assert!(C2::decode(&frame).is_err(), "decode: {what}");
    }
    // The same record in the right message is fine, and each message wants its own kind.
    let snapshot = record(RecordKind::Snapshot, cid(1), 1, 5, 10);
    assert!(C2::PutSnapshot { expected_snapshot_seq: 0, record: snapshot.clone() }.encode().is_ok());
    assert!(C2::Append { record: snapshot.clone() }.encode().is_err());
    assert!(C2::Presence { record: good.clone() }.encode().is_err());
    assert!(C2::Direct { to: None, record: good.clone() }.encode().is_err());
    assert!(S2::Direct { from: cid(1), record: good.clone() }.encode().is_err());
    assert!(S2::Record(StoredRecord { seq: 1, from: cid(1), ctr: 1, record: snapshot }).encode().is_err());
}

#[test]
fn fields_that_repeat_the_record_header_must_agree_with_it() {
    let rec = update(1, 5, 10);
    let wrong_sender = StoredRecord { seq: 1, from: cid(2), ctr: 5, record: rec.clone() };
    let wrong_ctr = StoredRecord { seq: 1, from: cid(1), ctr: 6, record: rec.clone() };
    for bad in [wrong_sender, wrong_ctr] {
        assert!(S2::Record(bad.clone()).encode().is_err());
        assert!(S2::Catchup { snapshot: None, records: vec![bad], head_seq: 1, more: false }.encode().is_err());
    }
    let snapshot = record(RecordKind::Snapshot, cid(1), 1, 5, 10);
    let wrong_cover = CatchupSnapshot { covers_seq: 6, record: snapshot };
    assert!(S2::Catchup { snapshot: Some(wrong_cover), records: vec![], head_seq: 6, more: false }.encode().is_err());
    let wrong = PresenceEntry { from: cid(2), record: record(RecordKind::Presence, cid(1), 1, 0, 1) };
    assert!(S2::PresenceBundle(vec![wrong]).encode().is_err());
    assert!(S2::Direct { from: cid(2), record: record(RecordKind::Direct, cid(1), 1, 0, 1) }.encode().is_err());
}

// ---- sizes and limits -----------------------------------------------------------------------

#[test]
fn records_are_bounded_by_their_kind() {
    let at_limit = |kind, limit| record(kind, cid(1), 1, 0, limit - SEAL_OVERHEAD);
    let one_more = |kind, limit| record(kind, cid(1), 1, 0, limit - SEAL_OVERHEAD + 1);
    assert!(C2::Append { record: at_limit(RecordKind::Update, MAX_RECORD_BYTES) }.encode().is_ok());
    assert!(C2::Append { record: one_more(RecordKind::Update, MAX_RECORD_BYTES) }.encode().is_err());
    assert!(C2::Presence { record: at_limit(RecordKind::Presence, MAX_PRESENCE_RECORD_BYTES) }.encode().is_ok());
    assert!(C2::Presence { record: one_more(RecordKind::Presence, MAX_PRESENCE_RECORD_BYTES) }.encode().is_err());
    let snapshot =
        |extra: usize| record(RecordKind::Snapshot, cid(1), 1, 0, MAX_SNAPSHOT_RECORD_BYTES - SEAL_OVERHEAD + extra);
    assert!(C2::PutSnapshot { expected_snapshot_seq: 0, record: snapshot(0) }.encode().is_ok());
    assert!(C2::PutSnapshot { expected_snapshot_seq: 0, record: snapshot(1) }.encode().is_err());
    // Decoding enforces the same limit as encoding.
    let frame = C2::Append { record: at_limit(RecordKind::Update, MAX_RECORD_BYTES) }.encode().unwrap();
    assert!(C2::decode(&frame).is_ok());
    let mut over = vec![0x10];
    over.extend_from_slice(&[0x81, 0x80, 0x80, 0x02]); // varint 4 MiB + 1
    over.extend(vec![0u8; MAX_RECORD_BYTES + 1]);
    assert!(matches!(C2::decode(&over), Err(DecodeError::FieldTooLong { .. })));
}

#[test]
fn the_frame_cap_holds_for_a_catchup_of_a_snapshot_and_records() {
    let big = |seq: u64| StoredRecord { seq, from: cid(1), ctr: seq, record: update(1, seq, 1 << 20) };
    let many: Vec<_> = (1..=MAX_CATCHUP_RECORDS as u64).map(big).collect();
    // 64 records of 1 MiB do not fit in one frame: the encoder refuses, so the relay must send fewer and set `more`.
    let too_big = S2::Catchup { snapshot: None, records: many, head_seq: 64, more: false };
    assert!(matches!(too_big.encode(), Err(EncodeError::FrameTooLarge { .. })));
    let fits = S2::Catchup { snapshot: None, records: (1..=8).map(big).collect(), head_seq: 8, more: true };
    assert!(S2::decode(&fits.encode().unwrap()).is_ok());
    // One more than the record count limit is refused whatever the size.
    let small: Vec<_> = (1..=MAX_CATCHUP_RECORDS as u64 + 1).map(|s| stored(s, 1, s)).collect();
    assert!(S2::Catchup { snapshot: None, records: small, head_seq: 65, more: false }.encode().is_err());
}

#[test]
fn lists_are_bounded() {
    let peers: Vec<_> = (0..=MAX_PEERS).map(|i| ClientId::from_bytes([i as u8; 16])).collect();
    let mut joined = server_samples()[0].clone();
    if let S2::Joined { peers: p, .. } = &mut joined {
        *p = peers[..MAX_PEERS].to_vec();
    }
    assert!(S2::decode(&joined.encode().unwrap()).is_ok());
    if let S2::Joined { peers: p, .. } = &mut joined {
        *p = peers;
    }
    assert!(joined.encode().is_err());

    let entry = |n: u8| PresenceEntry { from: cid(n), record: record(RecordKind::Presence, cid(n), 1, 0, 1) };
    let bundle = |n: usize| S2::PresenceBundle((0..n).map(|i| entry(i as u8)).collect());
    assert!(S2::decode(&bundle(MAX_BUNDLE_ENTRIES).encode().unwrap()).is_ok());
    assert!(bundle(MAX_BUNDLE_ENTRIES + 1).encode().is_err());
}

#[test]
fn error_messages_and_proofs_and_captchas_are_bounded() {
    let long = "é".repeat(MAX_ERROR_MESSAGE_BYTES);
    let error = S2::error(E2::Malformed, long);
    assert_eq!(S2::decode(&error.encode().unwrap()).unwrap(), error);
    let too_long =
        S2::Error { code: E2::Malformed, message: "x".repeat(MAX_ERROR_MESSAGE_BYTES + 1), retry_after_ms: None };
    assert!(too_long.encode().is_err());

    let proof = |len: usize| Proof::new(ProofScheme::Token, vec![7; len]);
    assert!(J { proof: proof(MAX_PROOF_BYTES), ..J::default() }.msg().encode().is_ok());
    assert!(J { proof: proof(MAX_PROOF_BYTES + 1), ..J::default() }.msg().encode().is_err());
    let captcha = |len: usize| Some("c".repeat(len));
    assert!(J { captcha: captcha(MAX_CAPTCHA_BYTES), ..J::default() }.msg().encode().is_ok());
    assert!(J { captcha: captcha(MAX_CAPTCHA_BYTES + 1), ..J::default() }.msg().encode().is_err());
}

// ---- the Join handshake ---------------------------------------------------------------------

/// A v1 `Join` frame for a plaintext room.
fn v1_join(min: u16, max: u16) -> Vec<u8> {
    ClientMessage::Join {
        min_version: min,
        max_version: max,
        room: drafftink_protocol::RoomId::new("room").unwrap(),
        client_id: cid(1),
        auth_token: None,
        version: vec![],
        user: None,
    }
    .encode()
    .unwrap()
}

fn v2_join(min: u16, max: u16) -> Vec<u8> {
    join(min, max).encode().unwrap()
}

#[test]
fn the_first_five_bytes_of_a_join_are_the_same_in_v1_and_v2() {
    for (min, max) in [(1, 1), (2, 2), (1, 2), (1, 9), (0, 0)] {
        let (v1, v2) = (v1_join(min, max), v2_join(min, max));
        assert_eq!(v1[..5], v2[..5], "{min}..={max}");
        assert_eq!(v2[..5], [0x01, min as u8, 0, max as u8, 0]);
        let seen = JoinVersions::peek(&v2).unwrap();
        assert_eq!((seen.min, seen.max), (min, max));
        assert_eq!(JoinVersions::peek(&v1).unwrap(), seen);
    }
}

#[test]
fn the_versions_of_a_first_frame_are_read_without_reading_the_rest() {
    assert_eq!(JoinVersions::peek(&[]), Err(DecodeError::Empty));
    assert_eq!(JoinVersions::peek(&[0x10, 2, 0, 2, 0]), Err(DecodeError::UnknownTag(0x10)));
    assert!(matches!(JoinVersions::peek(&[1, 2, 0, 2]), Err(DecodeError::Truncated(_))));
    // Whatever follows the five bytes is not looked at.
    let garbage = [&[1u8, 2, 0, 2, 0][..], &[0xff; 40]].concat();
    assert_eq!(JoinVersions::peek(&garbage), Ok(JoinVersions { min: 2, max: 2 }));
}

#[test]
fn the_highest_common_version_is_negotiated() {
    let versions = |min, max| JoinVersions { min, max }.negotiate();
    assert_eq!((PROTOCOL_V1, PROTOCOL_V2, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION), (1, 2, 1, 2));
    assert_eq!(versions(1, 1), Some(1));
    assert_eq!(versions(2, 2), Some(2));
    assert_eq!(versions(1, 2), Some(2));
    assert_eq!(versions(1, 9), Some(2));
    assert_eq!(versions(0, 0), None);
    assert_eq!(versions(3, 5), None);
    assert_eq!(versions(2, 1), None);
    // A host that speaks only some of the versions.
    assert_eq!(negotiate_version_within(1, 2, PROTOCOL_V1..=PROTOCOL_V1), Some(1));
    assert_eq!(negotiate_version_within(2, 2, PROTOCOL_V1..=PROTOCOL_V1), None);
    assert_eq!(negotiate_version_within(1, 2, PROTOCOL_V2..=PROTOCOL_V2), Some(2));
    assert_eq!(negotiate_version_within(1, 1, PROTOCOL_V2..=PROTOCOL_V2), None);
}

#[test]
fn each_decoder_refuses_the_join_of_the_other_version_by_name() {
    // A plaintext Join to the v2 decoder, and a sealed Join to the v1 decoder.
    let err = C2::decode(&v1_join(1, 1)).unwrap_err();
    assert_eq!(err, DecodeError::WrongLayout { client_min: 1, client_max: 1, codec: 2 });
    let err = ClientMessage::decode(&v2_join(2, 2)).unwrap_err();
    assert_eq!(err, DecodeError::WrongLayout { client_min: 2, client_max: 2, codec: 1 });
    assert_eq!(err.error_code(), ErrorCode::UnsupportedVersion);
    assert!(err.to_string().contains("2..=2") && err.to_string().contains("version 1 codec"), "{err}");
    // Both are errors in the reply a host sends.
    assert!(matches!(
        ServerMessage::error_for_decode(&err),
        ServerMessage::Error { code: ErrorCode::UnsupportedVersion, .. }
    ));
}

#[test]
fn a_range_that_includes_the_version_of_the_decoder_is_read_in_its_layout() {
    for (min, max) in [(2, 2), (1, 2), (2, 9), (1, 9)] {
        assert!(C2::decode(&v2_join(min, max)).is_ok(), "{min}..={max}");
    }
    for (min, max) in [(1, 1), (1, 2), (1, 9)] {
        assert!(ClientMessage::decode(&v1_join(min, max)).is_ok(), "{min}..={max}");
    }
}

#[test]
fn the_v2_decoder_checks_the_range_before_reading_the_rest() {
    // A client of a later version may lay out the rest of its Join differently.
    let mut frame = vec![0x01];
    frame.extend_from_slice(&3u16.to_le_bytes());
    frame.extend_from_slice(&9u16.to_le_bytes());
    frame.extend_from_slice(b"\xff\xff\xff another layout");
    assert_eq!(
        C2::decode(&frame),
        Err(DecodeError::UnsupportedVersion { client_min: 3, client_max: 9, ours_min: 1, ours_max: 2 })
    );
    assert!(join(3, 2).encode().is_err(), "inverted range");
}

#[test]
fn a_sealed_only_relay_can_say_why_it_refuses_a_plaintext_join() {
    let reply = S2::error(
        E2::UnsupportedVersion,
        "This server only hosts end-to-end encrypted rooms. Use a room link that has a key.",
    );
    assert_eq!(S2::decode(&reply.encode().unwrap()).unwrap(), reply);
}

// ---- membership -----------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_room_id_of_a_known_token_is_the_known_one() {
    // From `crates/seal/tests/vectors/vectors.json`, made by another implementation: the membership token of the
    // room key 00 01 02 ... 1f, and SHA-256("drafft v1 room-id" || token)[..16].
    let token = MembershipToken::from_bytes([
        0x59, 0x65, 0xd9, 0xf4, 0x53, 0x7a, 0x27, 0xca, 0x24, 0x77, 0x4d, 0x2b, 0x5b, 0x12, 0x38, 0x66, 0x4d, 0xb9,
        0x0c, 0xdf, 0x05, 0x8d, 0xc4, 0x5b, 0x19, 0xb5, 0x34, 0x1e, 0x14, 0xc2, 0xa8, 0xb0,
    ]);
    let id = RoomId::from_token(&token);
    assert_eq!(hex(id.as_bytes()), "e019c641b339c0a98ce840b5e4f0cdbe");
    assert_eq!(id.to_string(), "4BnGQbM5wKmM6EC15PDNvg");
    assert_eq!("4BnGQbM5wKmM6EC15PDNvg".parse::<RoomId>().unwrap(), id);
}

#[test]
fn only_the_token_of_the_room_is_accepted() {
    let (token, id) = (token(), room());
    assert!(id.accepts(&Proof::token(&token)));
    let other = MembershipToken::from_bytes([1; 32]);
    assert!(!id.accepts(&Proof::token(&other)));
    assert!(!RoomId::from_token(&other).accepts(&Proof::token(&token)));
    // A token with any bit changed, one byte short, one byte long, empty.
    for bit in 0..256 {
        let mut bytes = *token.expose_bytes();
        bytes[bit / 8] ^= 1 << (bit % 8);
        assert!(!id.accepts(&Proof::new(ProofScheme::Token, bytes.to_vec())), "bit {bit}");
    }
    let bytes = token.expose_bytes();
    for wrong in [bytes[..31].to_vec(), [&bytes[..], &[0u8]].concat(), vec![], bytes[1..].to_vec(), vec![0; 32]] {
        assert!(!id.accepts(&Proof::new(ProofScheme::Token, wrong)));
    }
    // A short proof still travels, so that the relay can answer it with `Unauthorized`.
    let short = J { proof: Proof::new(ProofScheme::Token, bytes[..5].to_vec()), ..J::default() }.msg();
    assert_eq!(C2::decode(&short.encode().unwrap()).unwrap(), short);
}

#[test]
fn ids_are_unpadded_base64url_of_the_exact_length() {
    let id = room();
    let text = id.to_string();
    assert_eq!(text.len(), 22);
    assert_eq!(text.parse::<RoomId>().unwrap(), id);
    let name = ImageName::from_bytes([0xfb; 16]);
    assert_eq!(name.to_string().parse::<ImageName>().unwrap(), name);
    assert!(name.to_string().contains('-') || name.to_string().contains('_'), "url alphabet: {name}");
    for bad in [
        "",
        &text[..21],
        &format!("{text}A"),
        &format!("{}=", &text[..21]),
        &format!("{}+", &text[..21]),
        &format!("{}/", &text[..21]),
        &format!("{} ", &text[..21]),
        // The last character of 22 has two spare bits that must be zero.
        &format!("{}B", &text[..21]),
        "é2345678901234567890a",
    ] {
        assert!(bad.parse::<RoomId>().is_err() || bad == text, "{bad:?}");
    }
    assert!("4BnGQbM5wKmM6EC15PDNvh".parse::<RoomId>().is_err(), "non-canonical last character");
}

#[test]
fn secrets_do_not_show_in_debug() {
    let proof = Proof::token(&token());
    for shown in [format!("{proof:?}"), format!("{:?}", token()), format!("{:#?}", join(2, 2))] {
        assert!(!shown.contains("192") && !shown.contains(&hex(token().expose_bytes())), "{shown}");
        assert!(!shown.contains("193, 194"), "{shown}");
    }
    assert_eq!(format!("{:?}", token()), "MembershipToken(..)");
}

// ---- codes, constants and limits ------------------------------------------------------------

#[test]
fn error_codes_have_stable_numbers_and_share_them_with_v1() {
    for n in 0..=255u8 {
        if let Some(code) = E2::from_u8(n) {
            assert_eq!(code.as_u8(), n);
        }
    }
    let shared = [
        (E2::Malformed, ErrorCode::Malformed),
        (E2::UnsupportedVersion, ErrorCode::UnsupportedVersion),
        (E2::Unauthorized, ErrorCode::Unauthorized),
        (E2::Forbidden, ErrorCode::Forbidden),
        (E2::RoomFull, ErrorCode::RoomFull),
        (E2::TooLarge, ErrorCode::TooLarge),
        (E2::RateLimited, ErrorCode::RateLimited),
        (E2::Replaced, ErrorCode::Replaced),
        (E2::NotJoined, ErrorCode::NotJoined),
        (E2::AlreadyJoined, ErrorCode::AlreadyJoined),
        (E2::Unavailable, ErrorCode::Unavailable),
        (E2::Internal, ErrorCode::Internal),
    ];
    for (v2, v1) in shared {
        assert_eq!(v2.as_u8(), v1.as_u8(), "{v2:?}");
        assert_eq!((v2.closes_connection(), v2.retryable()), (v1.closes_connection(), v1.retryable()), "{v2:?}");
    }
    // The v1 enum is unchanged: its numbers stop at 14, and v1 clients cannot read the new codes.
    assert_eq!(ErrorCode::from_u8(15), None);
    let new = [(E2::RoomMissing, 15), (E2::WrongRoomKind, 16), (E2::Conflict, 17), (E2::DailyLimit, 18)];
    for (code, n) in new {
        assert_eq!((code.as_u8(), E2::from_u8(n)), (n, Some(code)));
    }
    assert_eq!(E2::from_u8(8), None, "ResyncRequired is v1 only");
    assert_eq!(E2::from_u8(12), None, "InvalidUpdate is v1 only");
    assert!(E2::RoomMissing.closes_connection() && !E2::Conflict.closes_connection());
    assert!(E2::DailyLimit.retryable() && !E2::Unauthorized.retryable());
}

#[test]
fn the_limits_have_the_values_of_the_design() {
    const MIB: usize = 1024 * 1024;
    assert_eq!(MAX_IMAGE_BYTES, 4 * MIB);
    assert_eq!(MAX_ROOM_IMAGE_BYTES, 50 * MIB as u64);
    assert_eq!(MAX_ROOM_IMAGES, 200);
    assert_eq!(MAX_FRAME_BYTES, 16 * MIB);
    assert_eq!(MAX_RECORD_BYTES, 4 * MIB);
    assert_eq!(MAX_SNAPSHOT_RECORD_BYTES, MAX_SYNC_BYTES);
    assert_eq!(MAX_SYNC_BYTES, 15 * MIB);
    assert_eq!(MAX_ROOM_STATE_BYTES, 32 * MIB as u64);
    assert_eq!(SELF_HOST_ROOM_STATE_BYTES, 256 * MIB as u64);
    assert_eq!(ROOM_TTL.as_secs(), 48 * 3600);
    assert_eq!(ROOM_LEAVE_GRACE.as_secs(), 60);
    assert_eq!(ROOM_RECHECK.as_secs(), 3600);
    assert_eq!(MAX_CATCHUP_RECORDS, 64);
    assert_eq!(CATCHUP_TARGET_BYTES, 2 * MIB);
    assert_eq!(ROOM_PRESENCE_BUDGET, 40);
    assert_eq!(PRESENCE_TICK_MS, 100);
    assert_eq!((MAX_PEERS_PER_ROOM, MAX_ROOM_UPDATES_PER_SEC), (64, 200));
    assert_eq!((PUT_SNAPSHOT_MIN_INTERVAL_MS, CATCHUP_MIN_INTERVAL_MS, MAX_DIRECT_PER_SEC), (10_000, 1000, 5));
    assert_eq!((KEEPALIVE_PING, KEEPALIVE_PONG), ("ping", "pong"));
    // 34 + 24 + 4 + 16. The design says 74 and leaves the length prefix of the plaintext out.
    assert_eq!((RECORD_HEADER_BYTES, RECORD_NONCE_BYTES, RECORD_LEN_PREFIX_BYTES, RECORD_TAG_BYTES), (34, 24, 4, 16));
    assert_eq!(SEAL_OVERHEAD, 78);
    assert_eq!(MAX_SEALED_IMAGE_BYTES, MAX_IMAGE_BYTES + 78);
    // The v1 limits did not move.
    assert_eq!((MAX_UPDATE_BYTES, MAX_VERSION_BYTES, MAX_PEERS), (4 * MIB, 256 * 1024, 256));
}

#[test]
fn the_record_header_is_34_bytes_with_the_documented_layout() {
    let header = RecordHeader::new(RecordKind::Snapshot, cid(0xab), 0x0102_0304_0506_0708, 9).unwrap();
    let bytes = header.to_bytes();
    assert_eq!(bytes.len(), RECORD_HEADER_BYTES);
    assert_eq!(bytes[..2], [1, 2], "version, kind");
    assert_eq!(bytes[2..18], [0xab; 16], "sender");
    assert_eq!(bytes[18..26], [8, 7, 6, 5, 4, 3, 2, 1], "ctr, little endian");
    assert_eq!(bytes[26..], [9, 0, 0, 0, 0, 0, 0, 0], "covers_seq");
    let record: Vec<u8> = bytes.into_iter().chain(vec![0; SEAL_OVERHEAD - RECORD_HEADER_BYTES]).collect();
    assert_eq!(RecordHeader::peek(&record), Ok(header));
    for kind in 1..=5u8 {
        assert_eq!(RecordKind::from_u8(kind).map(RecordKind::as_u8), Some(kind));
    }
    assert_eq!((RecordKind::from_u8(0), RecordKind::from_u8(6)), (None, None));
    // Only snapshots have a cover, and an image has no sender and no counter.
    assert!(RecordHeader::new(RecordKind::Update, cid(1), 1, 1).is_err());
    assert!(RecordHeader::new(RecordKind::Image, cid(1), 0, 0).is_err());
    assert!(RecordHeader::new(RecordKind::Image, ClientId::from_bytes([0; 16]), 1, 0).is_err());
    assert_eq!(RecordHeader::new(RecordKind::Image, ClientId::from_bytes([0; 16]), 0, 0), Ok(RecordHeader::image()));
    assert_eq!(RecordHeader::peek(&record[..SEAL_OVERHEAD - 1]), Err(RecordError::TooShort { len: SEAL_OVERHEAD - 1 }));
}

#[test]
fn the_limits_a_relay_announces_stay_under_the_hard_ceilings() {
    let joined = |limits| JD { limits, ..JD::default() }.msg();
    assert!(joined(RoomLimits::DEFAULT).encode().is_ok());
    let over = [
        RoomLimits { max_frame_bytes: MAX_FRAME_BYTES as u32 + 1, ..RoomLimits::DEFAULT },
        RoomLimits { max_record_bytes: MAX_RECORD_BYTES as u32 + 1, ..RoomLimits::DEFAULT },
        RoomLimits { max_image_bytes: MAX_IMAGE_BYTES as u32 + 1, ..RoomLimits::DEFAULT },
        RoomLimits { max_room_image_bytes: MAX_ROOM_IMAGE_BYTES + 1, ..RoomLimits::DEFAULT },
        RoomLimits { max_room_images: MAX_ROOM_IMAGES + 1, ..RoomLimits::DEFAULT },
    ];
    for limits in over {
        assert!(joined(limits).encode().is_err(), "{limits:?}");
    }
    let lower = RoomLimits { max_image_bytes: 2 * 1024 * 1024, max_room_images: 10, ..RoomLimits::DEFAULT };
    assert_eq!(S2::decode(&joined(lower).encode().unwrap()).unwrap(), joined(lower));
    let inverted = JD { snapshot_seq: 5, head_seq: 4, ..JD::default() }.msg();
    assert!(inverted.encode().is_err(), "a snapshot cannot be newer than the head");
}

#[test]
fn the_presence_budget_is_shared_out() {
    let share = RoomLimits::presence_share;
    assert_eq!((share(0), share(1), share(2)), (20, 20, 20));
    assert_eq!((share(4), share(8), share(30), share(40), share(64), share(1000)), (10, 5, 1, 1, 1, 1));
    // The whole room never sends more than the budget (until the floor of 1 per person applies).
    for peers in 1..=40usize {
        assert!(usize::from(share(peers)) * peers <= usize::from(ROOM_PRESENCE_BUDGET), "{peers} peers");
    }
}
