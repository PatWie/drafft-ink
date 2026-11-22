#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Sealing and opening: round trips, and everything that must be refused. Each refusal is an error value; none panics.

mod common;

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};
use common::*;
use drafftink_protocol::v2::{ImageName, RecordHeader, RecordKind};
use drafftink_protocol::{MAX_IMAGE_BYTES, MAX_RECORD_BYTES, MAX_SEALED_IMAGE_BYTES, SEAL_OVERHEAD};
use drafftink_seal::{AAD_LABEL, FILE_ID_BYTES, OpenError, Purpose, RoomKey, RoomKeys, SealError, SealedRecord};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

const KINDS: [RecordKind; 4] = [RecordKind::Update, RecordKind::Snapshot, RecordKind::Presence, RecordKind::Direct];

fn header(kind: RecordKind) -> RecordHeader {
    let covers = if kind == RecordKind::Snapshot { 41 } else { 0 };
    RecordHeader::new(kind, client(7), 1234, covers).unwrap()
}

fn seal(keys: &RoomKeys, kind: RecordKind, payload: &[u8]) -> SealedRecord {
    keys.seal(header(kind), payload).unwrap()
}

fn name_of(keys: &RoomKeys, n: u8) -> ImageName {
    keys.image_name(&[n; FILE_ID_BYTES])
}

#[test]
fn every_kind_round_trips_at_every_size() {
    let keys = keys(1);
    for kind in KINDS {
        for len in [0, 1, 15, 16, 17, 63, 64, 65, 1000, 60_000] {
            let payload: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let sealed = seal(&keys, kind, &payload);
            assert_eq!(sealed.as_bytes().len(), len + SEAL_OVERHEAD);
            assert_eq!(*sealed.header(), header(kind));
            // Through the bytes, as a relay would hand it on.
            let received = SealedRecord::from_bytes(sealed.as_bytes().to_vec()).unwrap();
            assert_eq!(keys.open(kind, &received).unwrap(), payload, "{kind:?} of {len} bytes");
        }
    }
}

#[test]
fn images_round_trip_under_their_name() {
    let keys = keys(1);
    let name = name_of(&keys, 1);
    let picture = vec![0x42; 5000];
    let sealed = keys.seal_image(&name, &picture).unwrap();
    assert_eq!(*sealed.header(), RecordHeader::image());
    assert_eq!(keys.open_image(&name, &sealed).unwrap(), picture);
}

#[test]
fn the_same_plaintext_seals_differently_each_time() {
    let keys = keys(1);
    let a = seal(&keys, RecordKind::Update, b"same");
    let b = seal(&keys, RecordKind::Update, b"same");
    assert_ne!(a.nonce(), b.nonce());
    assert_ne!(a.as_bytes(), b.as_bytes());
    assert_ne!(a.as_bytes()[34 + 24..], b.as_bytes()[34 + 24..], "the ciphertext differs too");
}

/// Whether `bytes`, taken as a record, is refused for every kind it could be opened as.
fn refused(keys: &RoomKeys, bytes: &[u8], opened_as: RecordKind) -> bool {
    match SealedRecord::from_bytes(bytes.to_vec()) {
        Err(_) => true,
        Ok(record) if opened_as == RecordKind::Image => keys.open_image(&name_of(keys, 1), &record).is_err(),
        Ok(record) => {
            // As the caller expects it, and as the (possibly changed) header says it is.
            keys.open(opened_as, &record).is_err() && keys.open(record.header().kind, &record).is_err()
        }
    }
}

#[test]
fn every_single_bit_flip_is_refused() {
    let keys = keys(1);
    for kind in KINDS {
        let sealed = seal(&keys, kind, b"twenty bytes of data");
        let original = sealed.as_bytes();
        for bit in 0..original.len() * 8 {
            let mut bytes = original.to_vec();
            bytes[bit / 8] ^= 1 << (bit % 8);
            assert!(refused(&keys, &bytes, kind), "{kind:?}: flipping bit {bit} was accepted");
        }
    }
    let name = name_of(&keys, 1);
    let sealed = keys.seal_image(&name, b"a small picture").unwrap();
    for bit in 0..sealed.as_bytes().len() * 8 {
        let mut bytes = sealed.as_bytes().to_vec();
        bytes[bit / 8] ^= 1 << (bit % 8);
        assert!(refused(&keys, &bytes, RecordKind::Image), "image: flipping bit {bit} was accepted");
    }
}

#[test]
fn bit_flips_of_a_large_record_are_refused() {
    let keys = keys(1);
    let sealed = seal(&keys, RecordKind::Snapshot, &vec![0x5a; 100_000]);
    let original = sealed.as_bytes();
    let bits = original.len() * 8;
    let mut rng = Rng(0x1234_5678);
    // The header and nonce (68 bytes) and the tag (16 bytes), and 1000 places in between.
    let edges = (0..68 * 8).chain(bits - 16 * 8..bits);
    let sampled = (0..1000).map(|_| rng.below(bits));
    for bit in edges.chain(sampled) {
        let mut bytes = original.to_vec();
        bytes[bit / 8] ^= 1 << (bit % 8);
        assert!(refused(&keys, &bytes, RecordKind::Snapshot), "flipping bit {bit} was accepted");
    }
}

#[test]
fn every_header_field_is_authenticated() {
    let keys = keys(1);
    for kind in KINDS {
        let sealed = seal(&keys, kind, b"payload");
        let original = sealed.as_bytes();
        // (name, byte range of the field in the record)
        let fields = [("version", 0..1), ("kind", 1..2), ("sender", 2..18), ("ctr", 18..26), ("covers_seq", 26..34)];
        for (field, range) in fields {
            for index in range {
                for flip in [0x01u8, 0x80, 0xff] {
                    let mut bytes = original.to_vec();
                    bytes[index] ^= flip;
                    assert!(refused(&keys, &bytes, kind), "{kind:?}: changing {field} byte {index} by {flip:#x}");
                }
            }
        }
        // Another kind with the rest of the header intact, for every kind.
        for other in [RecordKind::Update, RecordKind::Snapshot, RecordKind::Presence, RecordKind::Direct] {
            if other == kind {
                continue;
            }
            let mut bytes = original.to_vec();
            bytes[1] = other.as_u8();
            assert!(refused(&keys, &bytes, kind), "{kind:?} relabelled as {other:?}");
        }
    }
}

#[test]
fn the_nonce_and_the_tag_are_authenticated() {
    let keys = keys(1);
    let sealed = seal(&keys, RecordKind::Update, b"payload");
    let len = sealed.as_bytes().len();
    for index in (34..58).chain(len - 16..len) {
        let mut bytes = sealed.as_bytes().to_vec();
        bytes[index] ^= 0x10;
        assert!(refused(&keys, &bytes, RecordKind::Update), "byte {index}");
    }
}

#[test]
fn truncation_at_every_length_is_refused() {
    let keys = keys(1);
    for kind in KINDS {
        let sealed = seal(&keys, kind, b"some payload bytes");
        let bytes = sealed.as_bytes();
        for len in 0..bytes.len() {
            assert!(refused(&keys, &bytes[..len], kind), "{kind:?} cut to {len} bytes");
        }
    }
    let name = name_of(&keys, 1);
    let sealed = keys.seal_image(&name, b"picture").unwrap();
    for len in 0..sealed.as_bytes().len() {
        assert!(refused(&keys, &sealed.as_bytes()[..len], RecordKind::Image), "image cut to {len} bytes");
    }
}

#[test]
fn bytes_added_at_the_end_are_refused() {
    let keys = keys(1);
    let sealed = seal(&keys, RecordKind::Update, b"payload");
    for extra in [vec![0u8], vec![0u8; 16], vec![0xff; 3]] {
        let bytes = [sealed.as_bytes(), &extra].concat();
        assert!(refused(&keys, &bytes, RecordKind::Update));
    }
}

#[test]
fn a_wrong_key_refuses_every_kind() {
    let right = keys(1);
    let wrong = keys(2);
    for kind in KINDS {
        let sealed = seal(&right, kind, b"secret");
        assert_eq!(wrong.open(kind, &sealed), Err(OpenError::NotAuthentic), "{kind:?}");
    }
    let name = name_of(&right, 1);
    let sealed = right.seal_image(&name, b"secret").unwrap();
    assert_eq!(wrong.open_image(&name, &sealed), Err(OpenError::NotAuthentic));
    assert_eq!(wrong.open_image(&name_of(&wrong, 1), &sealed), Err(OpenError::NotAuthentic));
}

/// A key that differs from `RoomKey::from_bytes([1; 32])` in one bit is a different room and opens nothing.
#[test]
fn a_key_one_bit_away_refuses() {
    let mut bytes = [1u8; 32];
    bytes[31] ^= 1;
    let near = RoomKeys::new(&RoomKey::from_bytes(bytes));
    let sealed = seal(&keys(1), RecordKind::Update, b"secret");
    assert_eq!(near.open(RecordKind::Update, &sealed), Err(OpenError::NotAuthentic));
}

#[test]
fn a_record_cannot_be_replayed_in_another_room() {
    let (room_a, room_b) = (keys(1), keys(2));
    assert_ne!(room_a.room_id(), room_b.room_id());
    for kind in KINDS {
        let sealed = seal(&room_a, kind, b"for room A");
        assert_eq!(room_b.open(kind, &sealed), Err(OpenError::NotAuthentic), "{kind:?}");
    }
}

#[test]
fn a_record_cannot_be_replayed_as_another_kind() {
    let keys = keys(1);
    let update = seal(&keys, RecordKind::Update, b"an update");
    let presence = seal(&keys, RecordKind::Presence, b"a cursor");
    // The caller asks for the kind it expects.
    assert!(matches!(keys.open(RecordKind::Snapshot, &update), Err(OpenError::WrongKind { .. })));
    assert!(matches!(keys.open(RecordKind::Direct, &presence), Err(OpenError::WrongKind { .. })));
    // A relay relabels the record. The header is in the associated data, so the cipher refuses it, even for the
    // two kinds that share a key.
    for (record, from, to) in [
        (&update, RecordKind::Update, RecordKind::Snapshot),
        (&update, RecordKind::Update, RecordKind::Presence),
        (&presence, RecordKind::Presence, RecordKind::Direct),
        (&presence, RecordKind::Presence, RecordKind::Update),
    ] {
        let mut bytes = record.as_bytes().to_vec();
        bytes[1] = to.as_u8();
        let relabelled = SealedRecord::from_bytes(bytes).unwrap();
        assert_eq!(keys.open(to, &relabelled), Err(OpenError::NotAuthentic), "{from:?} as {to:?}");
    }
}

#[test]
fn an_image_is_bound_to_its_name() {
    let keys = keys(1);
    let (name, other) = (name_of(&keys, 1), name_of(&keys, 2));
    let sealed = keys.seal_image(&name, b"picture").unwrap();
    assert_eq!(keys.open_image(&other, &sealed), Err(OpenError::NotAuthentic));
    let mut one_bit = *name.as_bytes();
    one_bit[0] ^= 1;
    assert_eq!(keys.open_image(&ImageName::from_bytes(one_bit), &sealed), Err(OpenError::NotAuthentic));
    // An image is not an update, and an update is not an image.
    assert_eq!(keys.open(RecordKind::Image, &sealed), Err(OpenError::ImageKind));
    assert!(matches!(keys.open(RecordKind::Update, &sealed), Err(OpenError::WrongKind { .. })));
    let update = seal(&keys, RecordKind::Update, b"update");
    assert!(matches!(keys.open_image(&name, &update), Err(OpenError::WrongKind { .. })));
    // An update body relabelled as an image: the image key is another key.
    let mut bytes = update.as_bytes().to_vec();
    bytes[1] = RecordKind::Image.as_u8();
    assert!(refused(&keys, &bytes, RecordKind::Image));
}

#[test]
fn the_name_of_an_image_depends_on_the_room_and_the_picture() {
    let (a, b) = (keys(1), keys(2));
    assert_eq!(name_of(&a, 5), name_of(&a, 5));
    assert_ne!(name_of(&a, 5), name_of(&a, 6));
    assert_ne!(name_of(&a, 5), name_of(&b, 5), "the same picture has unrelated names in two rooms");
}

#[test]
fn an_image_is_not_sealed_as_a_record() {
    let keys = keys(1);
    assert_eq!(keys.seal(RecordHeader::image(), b"x").unwrap_err(), SealError::ImageKind);
}

#[test]
fn sizes_at_the_limits_are_accepted_and_one_more_byte_is_not() {
    let keys = keys(1);
    let name = name_of(&keys, 1);

    let picture = vec![0u8; MAX_IMAGE_BYTES];
    let sealed = keys.seal_image(&name, &picture).unwrap();
    assert_eq!(sealed.as_bytes().len(), MAX_SEALED_IMAGE_BYTES);
    assert_eq!(keys.open_image(&name, &sealed).unwrap().len(), MAX_IMAGE_BYTES);
    assert!(matches!(keys.seal_image(&name, &vec![0u8; MAX_IMAGE_BYTES + 1]), Err(SealError::TooLarge { .. })));

    let update = vec![0u8; MAX_RECORD_BYTES - SEAL_OVERHEAD];
    assert_eq!(seal(&keys, RecordKind::Update, &update).as_bytes().len(), MAX_RECORD_BYTES);
    let err = keys.seal(header(RecordKind::Update), &vec![0u8; update.len() + 1]).unwrap_err();
    assert!(matches!(err, SealError::TooLarge { kind: RecordKind::Update, .. }), "{err}");
    assert!(err.to_string().contains("bytes"), "plain words: {err}");
}

/// Seals `plaintext` as the update with the header of `header()`, as only a holder of the key could. The bytes of
/// a record that is too short to be a record come back as an error.
fn seal_by_hand(room: u8, plaintext: &[u8]) -> Result<SealedRecord, drafftink_protocol::v2::RecordError> {
    let key = RoomKey::from_bytes([room; 32]);
    let keys = RoomKeys::new(&key);
    let header = header(RecordKind::Update);
    let aad = [AAD_LABEL, keys.room_id().as_bytes(), &header.to_bytes()].concat();
    let cipher = XChaCha20Poly1305::new(&Key::from(*key.subkey(Purpose::Content).expose_bytes()));
    let nonce = [9u8; 24];
    let ciphertext = cipher.encrypt(&XNonce::from(nonce), Payload { msg: plaintext, aad: &aad }).unwrap();
    SealedRecord::from_bytes([header.to_bytes().as_slice(), &nonce, &ciphertext].concat())
}

#[test]
fn an_authentic_record_with_a_bad_inner_length_is_malformed() {
    let keys = keys(1);
    let with_prefix = |len: u32, rest: &[u8]| [&len.to_le_bytes()[..], rest].concat();
    // Zero padding after the payload is allowed (it is how padding will work), and cut off.
    let padded = seal_by_hand(1, &with_prefix(3, &[7, 8, 9, 0, 0, 0, 0])).unwrap();
    assert_eq!(keys.open(RecordKind::Update, &padded).unwrap(), [7, 8, 9]);
    // Everything else is refused: a length longer than the data, a length of any size, padding that is not zero.
    for bad in [with_prefix(4, &[7, 8, 9]), with_prefix(u32::MAX, &[7]), with_prefix(1, &[7, 0, 1])] {
        let record = seal_by_hand(1, &bad).unwrap();
        assert_eq!(keys.open(RecordKind::Update, &record), Err(OpenError::Malformed), "{bad:?}");
    }
    // A plaintext with no room for a length prefix makes a record that is shorter than any record: refused on arrival.
    for short in [vec![], vec![1, 0, 0]] {
        assert!(seal_by_hand(1, &short).is_err(), "{short:?}");
    }
}

#[test]
fn the_errors_are_plain_words() {
    let keys = keys(1);
    let sealed = seal(&keys, RecordKind::Update, b"x");
    let err = self::keys(2).open(RecordKind::Update, &sealed).unwrap_err();
    assert!(err.to_string().starts_with("This record was not sealed with this room's key"), "{err}");
}
