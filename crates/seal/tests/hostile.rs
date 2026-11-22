#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Every input is hostile: random bytes, random mutations and random headers never panic, and nothing that was not
//! sealed with the key opens (compare `crates/collab/tests/hostile.rs`).

mod common;

use common::*;
use drafftink_protocol::v2::{RecordHeader, RecordKind};
use drafftink_seal::{RoomKeys, SealedRecord};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

const ALL_KINDS: [RecordKind; 5] =
    [RecordKind::Update, RecordKind::Snapshot, RecordKind::Presence, RecordKind::Direct, RecordKind::Image];

/// Opens `bytes` as every kind. Nothing may panic, and nothing may open.
fn assert_refused(keys: &RoomKeys, bytes: Vec<u8>) {
    let Ok(record) = SealedRecord::from_bytes(bytes) else { return };
    for kind in ALL_KINDS {
        assert!(keys.open(kind, &record).is_err(), "{:?} opened as {kind:?}", record.header());
    }
    assert!(keys.open_image(&keys.image_name(&[0; 32]), &record).is_err());
}

#[test]
fn random_bytes_never_open() {
    let keys = keys(1);
    let mut rng = Rng(0xdead_beef);
    for _ in 0..5000 {
        let len = rng.below(400);
        assert_refused(&keys, rng.bytes(len));
    }
}

#[test]
fn random_bytes_behind_a_valid_header_never_open() {
    let keys = keys(1);
    let mut rng = Rng(0xfeed_f00d);
    for _ in 0..5000 {
        let kind = ALL_KINDS[rng.below(ALL_KINDS.len())];
        let header = if kind == RecordKind::Image {
            RecordHeader::image()
        } else {
            let covers = if kind == RecordKind::Snapshot { rng.next() } else { 0 };
            RecordHeader::new(kind, client(rng.below(256) as u8), rng.next(), covers).unwrap()
        };
        let extra = rng.below(300);
        let body = rng.bytes(24 + 16 + extra);
        assert_refused(&keys, [header.to_bytes().to_vec(), body].concat());
    }
}

#[test]
fn many_mutations_of_a_valid_record_never_open() {
    let keys = keys(1);
    let sealed = keys.seal(RecordHeader::new(RecordKind::Update, client(3), 9, 0).unwrap(), &[0x55; 200]).unwrap();
    let mut rng = Rng(0xabad_1dea);
    for _ in 0..5000 {
        let mut bytes = sealed.as_bytes().to_vec();
        for _ in 0..1 + rng.below(4) {
            match rng.below(4) {
                0 => {
                    let at = rng.below(bytes.len());
                    bytes[at] = rng.next() as u8;
                }
                1 => {
                    let at = rng.below(bytes.len());
                    bytes.insert(at, rng.next() as u8);
                }
                2 => {
                    let at = rng.below(bytes.len());
                    bytes.remove(at);
                }
                _ => {
                    let (a, b) = (rng.below(bytes.len()), rng.below(bytes.len()));
                    bytes.swap(a, b);
                }
            }
            if bytes.is_empty() {
                break;
            }
        }
        if bytes != sealed.as_bytes() {
            assert_refused(&keys, bytes);
        }
    }
}

#[test]
fn a_record_is_not_lost_in_the_noise() {
    // The control for the tests above: the unchanged record opens.
    let keys = keys(1);
    let sealed = keys.seal(RecordHeader::new(RecordKind::Update, client(3), 9, 0).unwrap(), &[0x55; 200]).unwrap();
    let again = SealedRecord::from_bytes(sealed.as_bytes().to_vec()).unwrap();
    assert_eq!(keys.open(RecordKind::Update, &again).unwrap(), vec![0x55; 200]);
}
