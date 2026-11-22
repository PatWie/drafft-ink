#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Known answers. The standard vectors check the primitives (and the wasm build of them). The vectors in
//! `vectors/vectors.json` were made by another implementation (`vectors/README.md`) and check our use of them.
//! The same file runs natively and under `wasm32` (`wasm-bindgen-test`), so both agree byte for byte.

mod common;

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};
use common::*;
use drafftink_protocol::v2::{ImageName, RecordHeader, RecordKind};
use drafftink_seal::{Purpose, RoomKeys, RoomLink, SealedRecord};
use hkdf::Hkdf;
use serde_json::Value;
use sha2::Sha256;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

const VECTORS: &str = include_str!("vectors/vectors.json");

fn vectors() -> Value {
    serde_json::from_str(VECTORS).unwrap()
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap()
}

/// draft-irtf-cfrg-xchacha-03, appendix A.3.1: AEAD_XCHACHA20_POLY1305.
#[test]
fn xchacha20_poly1305_matches_the_draft() {
    let plaintext = unhex(
        "4c616469657320616e642047656e746c656d656e206f662074686520636c6173\
         73206f66202739393a204966204920636f756c64206f6666657220796f75206f\
         6e6c79206f6e652074697020666f7220746865206675747572652c2073756e73\
         637265656e20776f756c642062652069742e",
    );
    let aad = unhex("50515253c0c1c2c3c4c5c6c7");
    let key = unhex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
    let nonce = unhex("404142434445464748494a4b4c4d4e4f5051525354555657");
    let ciphertext = unhex(
        "bd6d179d3e83d43b9576579493c0e939572a1700252bfaccbed2902c21396cbb\
         731c7f1b0b4aa6440bf3a82f4eda7e39ae64c6708c54c216cb96b72e1213b452\
         2f8c9ba40db5d945b11b69b982c1bb9e3f3fac2bc369488f76b2383565d3fff9\
         21f9664c97637da9768812f615c68b13b52e",
    );
    let tag = unhex("c0875924c1c7987947deafd8780acf49");

    let cipher = XChaCha20Poly1305::new(&Key::try_from(key.as_slice()).unwrap());
    let nonce = XNonce::try_from(nonce.as_slice()).unwrap();
    let sealed = cipher.encrypt(&nonce, Payload { msg: &plaintext, aad: &aad }).unwrap();
    assert_eq!(sealed, [ciphertext, tag].concat());
    assert_eq!(cipher.decrypt(&nonce, Payload { msg: &sealed, aad: &aad }).unwrap(), plaintext);
}

struct Rfc5869 {
    ikm: Vec<u8>,
    salt: Vec<u8>,
    info: Vec<u8>,
    prk: &'static str,
    okm: &'static str,
}

/// RFC 5869, appendix A, the three SHA-256 test cases (the others use SHA-1).
#[test]
fn hkdf_sha256_matches_rfc_5869() {
    let range = |from: u8, to: u8| (from..=to).collect::<Vec<u8>>();
    let cases = [
        Rfc5869 {
            ikm: vec![0x0b; 22],
            salt: range(0x00, 0x0c),
            info: range(0xf0, 0xf9),
            prk: "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5",
            okm: "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865",
        },
        Rfc5869 {
            ikm: range(0x00, 0x4f),
            salt: range(0x60, 0xaf),
            info: range(0xb0, 0xff),
            prk: "06a6b88c5853361a06104c9ceb35b45cef760014904671014a193f40c15fc244",
            okm: "b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c59045a99cac7827271cb41c65e590e09\
                  da3275600c2f09b8367793a9aca3db71cc30c58179ec3e87c14c01d5c1f3434f1d87",
        },
        Rfc5869 {
            ikm: vec![0x0b; 22],
            salt: vec![],
            info: vec![],
            prk: "19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04",
            okm: "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8",
        },
    ];
    for (n, case) in cases.iter().enumerate() {
        // An empty salt and an absent salt are the same in RFC 5869 (a string of zeros).
        let salt = (!case.salt.is_empty()).then_some(case.salt.as_slice());
        let (prk, hkdf) = Hkdf::<Sha256>::extract(salt, &case.ikm);
        assert_eq!(hex(&prk), case.prk, "case {}: PRK", n + 1);
        let mut okm = vec![0u8; case.okm.len() / 2];
        hkdf.expand(&case.info, &mut okm).unwrap();
        assert_eq!(hex(&okm), case.okm, "case {}: OKM", n + 1);
    }
}

#[test]
fn every_label_derives_the_known_subkey() {
    let v = vectors();
    let key = vector_key();
    assert_eq!(hex(key.expose_bytes()), text(&v, "root_key"));
    let purposes = [
        ("content", Purpose::Content),
        ("presence", Purpose::Presence),
        ("image", Purpose::Image),
        ("image-name", Purpose::ImageName),
        ("membership", Purpose::Membership),
    ];
    for (name, purpose) in purposes {
        let entry = &v["subkeys"][name];
        assert_eq!(purpose.label(), text(entry, "info").as_bytes(), "{name}: label");
        assert_eq!(hex(key.subkey(purpose).expose_bytes()), text(entry, "key"), "{name}: key");
    }
    assert_eq!(drafftink_seal::HKDF_SALT, text(&v, "hkdf_salt").as_bytes());
}

#[test]
fn the_token_the_room_id_and_the_link_are_known() {
    let v = vectors();
    let key = vector_key();
    assert_eq!(hex(key.membership_token().expose_bytes()), text(&v, "token"));
    assert_eq!(hex(key.room_id().as_bytes()), text(&v, "room_id"));
    assert_eq!(key.room_id().to_string(), text(&v, "room_id_base64url"));
    let link = RoomLink::new(key);
    assert_eq!(link.fragment().as_str(), text(&v, "link_fragment"));
    let parsed = RoomLink::parse(text(&v, "link_fragment")).unwrap();
    assert_eq!(hex(parsed.key().expose_bytes()), text(&v, "root_key"));
    assert_eq!(parsed.id().to_string(), text(&v, "room_id_base64url"));
}

#[test]
fn the_keyed_image_name_is_known() {
    let v = vectors();
    let keys = RoomKeys::new(&vector_key());
    let name = keys.image_name(&unhex_array(text(&v["image"], "file_id")));
    assert_eq!(hex(name.as_bytes()), text(&v["image"], "name"));
    assert_eq!(name.to_string(), text(&v["image"], "name_base64url"));
}

#[test]
fn every_sealed_record_is_known_and_opens() {
    let v = vectors();
    let keys = RoomKeys::new(&vector_key());
    let records = v["records"].as_array().unwrap();
    assert_eq!(records.len(), 6, "one record of each kind, and an update with no payload");
    for r in records {
        let label = text(r, "label");
        let payload = unhex(text(r, "payload"));
        let expected = unhex(text(r, "record"));
        let mut nonce = FixedNonce(unhex_array(text(r, "nonce")));
        let kind = RecordKind::from_u8(r["kind"].as_u64().unwrap() as u8).unwrap();

        let sealed = if kind == RecordKind::Image {
            let name = ImageName::from_bytes(unhex_array(text(r, "name")));
            keys.seal_image_with(&mut nonce, &name, &payload).unwrap()
        } else {
            let header =
                RecordHeader::new(kind, client_from(r), r["ctr"].as_u64().unwrap(), r["covers_seq"].as_u64().unwrap())
                    .unwrap();
            keys.seal_with(&mut nonce, header, &payload).unwrap()
        };
        assert_eq!(hex(sealed.as_bytes()), hex(&expected), "{label}: sealing");

        let received = SealedRecord::from_bytes(expected).unwrap();
        let opened = if kind == RecordKind::Image {
            keys.open_image(&ImageName::from_bytes(unhex_array(text(r, "name"))), &received)
        } else {
            keys.open(kind, &received)
        };
        assert_eq!(opened.unwrap(), payload, "{label}: opening");
    }
}

fn client_from(record: &Value) -> drafftink_protocol::ClientId {
    drafftink_protocol::ClientId::from_bytes(unhex_array(text(record, "sender")))
}

#[test]
fn a_record_of_the_vectors_does_not_open_in_another_room() {
    let v = vectors();
    let record = SealedRecord::from_bytes(unhex(text(&v["records"][0], "record"))).unwrap();
    assert!(keys(9).open(RecordKind::Update, &record).is_err());
}
