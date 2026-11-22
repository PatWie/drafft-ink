#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The frames that the TypeScript tests send to the Worker: real sealed records with a fixed room key and known
//! nonces, so the file is the same on every machine. `committed_fixtures_are_current` fails when the code that
//! makes the frames changed and the file did not; `cargo test -p drafftink-worker --test fixtures -- --ignored
//! regenerate` writes the file again.

use std::fmt::Write as _;
use std::path::PathBuf;

use base64ct::{Base64, Base64Url, Base64UrlUnpadded, Encoding};
use drafftink_protocol::v2::{ClientMessage, JoinMode, Proof, RecordHeader, RecordKind};
use drafftink_protocol::{ClientId, RECORD_NONCE_BYTES};
use drafftink_seal::{NonceSource, Purpose, RandomError, RoomKey, RoomKeys};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The text that must never reach the relay's storage in the clear.
const MARKER: &str = "CANARY-7f3a91c2";
/// A person's name in a presence record.
const USER_NAME: &str = "Zelda-Canary";

/// Nonces 1, 2, 3, ...: known, never repeated within one file.
struct Counting(u8);

impl NonceSource for Counting {
    fn next_nonce(&mut self) -> Result<[u8; RECORD_NONCE_BYTES], RandomError> {
        self.0 += 1;
        Ok([self.0; RECORD_NONCE_BYTES])
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn client(n: u8) -> ClientId {
    ClientId::from_bytes([n; 16])
}

struct Fixtures {
    key: RoomKey,
    keys: RoomKeys,
    nonces: Counting,
}

impl Fixtures {
    fn new(seed: u8) -> Self {
        let key = RoomKey::from_bytes([seed; 32]);
        let keys = RoomKeys::new(&key);
        Fixtures { key, keys, nonces: Counting(0) }
    }

    fn join(&self, who: ClientId, mode: JoinMode, last_seq: u64, proof_of: &RoomKeys) -> String {
        let message = ClientMessage::Join {
            min_version: 2,
            max_version: 2,
            room: self.keys.room_id(),
            client_id: who,
            mode,
            proof: Proof::token(proof_of.membership_token()),
            last_seq,
            captcha: None,
        };
        hex(&message.encode().unwrap_or_default())
    }

    fn sealed(&mut self, kind: RecordKind, who: ClientId, ctr: u64, covers: u64, payload: &[u8]) -> Vec<u8> {
        let header = RecordHeader::new(kind, who, ctr, covers).unwrap();
        self.keys.seal_with(&mut self.nonces, header, payload).unwrap().into_bytes()
    }

    fn append(&mut self, who: ClientId, ctr: u64, text: &str) -> String {
        let record = self.sealed(RecordKind::Update, who, ctr, 0, text.as_bytes());
        hex(&ClientMessage::Append { record }.encode().unwrap())
    }

    fn subkey_info(&self) -> Value {
        let one = |purpose| {
            let bytes = *self.key.subkey(purpose).expose_bytes();
            encodings(&bytes)
        };
        json!({
            "content": one(Purpose::Content),
            "presence": one(Purpose::Presence),
            "image": one(Purpose::Image),
            "imageName": one(Purpose::ImageName),
        })
    }
}

/// The ways a secret may show up in storage: raw bytes, hex, base64 and base64url (padded and not).
fn encodings(bytes: &[u8]) -> Value {
    json!({
        "hex": hex(bytes),
        "base64": Base64::encode_string(bytes),
        "base64url": Base64Url::encode_string(bytes),
        "base64urlNoPad": Base64UrlUnpadded::encode_string(bytes),
    })
}

/// The whole fixtures file.
fn build() -> Value {
    let mut room = Fixtures::new(0x42);
    let stranger = Fixtures::new(0x77);
    let (a, b, c) = (client(0xA1), client(0xB2), client(0xC3));

    let append_a1 = room.append(a, 1, &format!("update from A: {MARKER}"));
    let append_a2 = room.append(a, 2, "plain update from A");
    let append_b1 = room.append(b, 1, &format!("update from B: {MARKER}"));
    let presence = {
        let payload = format!("{{\"name\":\"{USER_NAME}\",\"cursor\":[10,20]}}");
        let record = room.sealed(RecordKind::Presence, a, 3, 0, payload.as_bytes());
        hex(&ClientMessage::Presence { record }.encode().unwrap())
    };
    let snapshot = {
        let record = room.sealed(RecordKind::Snapshot, a, 4, 2, format!("snapshot {MARKER}").as_bytes());
        hex(&ClientMessage::PutSnapshot { expected_snapshot_seq: 0, record }.encode().unwrap())
    };
    // The seed of an empty room: expects no snapshot, covers sequence number 0.
    let seed = {
        let record = room.sealed(RecordKind::Snapshot, a, 5, 0, format!("seed {MARKER}").as_bytes());
        hex(&ClientMessage::PutSnapshot { expected_snapshot_seq: 0, record }.encode().unwrap())
    };

    let png: Vec<u8> = [b"\x89PNG\r\n\x1a\n".as_slice(), format!("{MARKER} pixels").as_bytes()].concat();
    let file_id: [u8; 32] = Sha256::digest(&png).into();
    let image_name = room.keys.image_name(&file_id);
    let sealed_image = room.keys.seal_image_with(&mut room.nonces, &image_name, &png).unwrap().into_bytes();

    let join = |who, mode, last| room.join(who, mode, last, &room.keys);
    json!({
        "about": "Generated by apps/drafftink-worker/tests/fixtures.rs. Do not edit by hand.",
        "marker": MARKER,
        "userName": USER_NAME,
        "room": {
            "id": room.keys.room_id().to_string(),
            "key": encodings(room.key.expose_bytes()),
            "token": encodings(room.keys.membership_token().expose_bytes()),
            "subkeys": room.subkey_info(),
        },
        "stranger": {
            "id": stranger.keys.room_id().to_string(),
            "token": encodings(stranger.keys.membership_token().expose_bytes()),
        },
        "clients": { "a": hex(a.as_bytes()), "b": hex(b.as_bytes()), "c": hex(c.as_bytes()) },
        "image": {
            "name": image_name.to_string(),
            "fileId": hex(&file_id),
            "pngSha256": hex(&file_id),
            "pngHex": hex(&png),
            "sealedHex": hex(&sealed_image),
        },
        "frames": {
            "joinCreateA": join(a, JoinMode::Create, 0),
            "joinOpenA": join(a, JoinMode::Open, 0),
            "joinOpenB": join(b, JoinMode::Open, 0),
            "joinOpenC": join(c, JoinMode::Open, 0),
            "joinOpenAFrom2": join(a, JoinMode::Open, 2),
            "joinOpenAhead": join(a, JoinMode::Open, 99),
            "joinStranger": room.join(a, JoinMode::Open, 0, &stranger.keys),
            "appendA1": append_a1,
            "appendA2": append_a2,
            "appendB1": append_b1,
            "presenceA": presence,
            "snapshotCovers2": snapshot,
            "seedSnapshot": seed,
            "catchup0": hex(&ClientMessage::Catchup { after_seq: 0 }.encode().unwrap()),
            "leave": hex(&ClientMessage::Leave.encode().unwrap()),
            "deleteRoom": hex(&ClientMessage::DeleteRoom.encode().unwrap()),
        },
        "hostile": ["", "00", "ff", "01", "0102030405060708090a", "10ffffffff", "81", &"ab".repeat(300)],
    })
}

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/frames.json")
}

fn render() -> String {
    let mut text = serde_json::to_string_pretty(&build()).unwrap();
    text.push('\n');
    text
}

#[test]
fn committed_fixtures_are_current() {
    let committed = std::fs::read_to_string(path()).expect("test/fixtures/frames.json exists");
    assert_eq!(
        committed,
        render(),
        "test/fixtures/frames.json is stale: run `cargo test -p drafftink-worker --test fixtures -- --ignored regenerate`"
    );
}

#[test]
#[ignore = "writes test/fixtures/frames.json"]
fn regenerate() {
    std::fs::write(path(), render()).unwrap();
}

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2).map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap()).collect()
}

#[test]
fn sealed_frames_and_the_sealed_image_do_not_hold_the_canary_in_the_clear() {
    let value = build();
    let mut sealed: Vec<(String, Vec<u8>)> = value["frames"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, text)| (name.clone(), from_hex(text.as_str().unwrap())))
        .collect();
    sealed.push(("image".into(), from_hex(value["image"]["sealedHex"].as_str().unwrap())));
    for (name, bytes) in &sealed {
        for needle in [MARKER.as_bytes(), USER_NAME.as_bytes()] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle),
                "{name} holds {:?}",
                String::from_utf8_lossy(needle)
            );
        }
    }
    // The plaintext that the canary hides is really in the png.
    let png = from_hex(value["image"]["pngHex"].as_str().unwrap());
    assert!(png.windows(MARKER.len()).any(|w| w == MARKER.as_bytes()));
}
