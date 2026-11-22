#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Random nonces. A repeated nonce under one key breaks XChaCha20-Poly1305, so the source is checked directly.

mod common;

use common::*;
use drafftink_protocol::RECORD_NONCE_BYTES;
use drafftink_protocol::v2::{RecordHeader, RecordKind};
use drafftink_seal::{NonceSource, OsNonces};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

fn header() -> RecordHeader {
    RecordHeader::new(RecordKind::Presence, client(1), 1, 0).unwrap()
}

#[test]
fn a_thousand_seals_of_one_payload_have_no_nonce_or_ciphertext_in_common() {
    let keys = keys(1);
    let records: Vec<_> = (0..1000).map(|_| keys.seal(header(), b"same payload").unwrap()).collect();
    let mut nonces: Vec<_> = records.iter().map(|r| *r.nonce()).collect();
    nonces.sort_unstable();
    nonces.dedup();
    assert_eq!(nonces.len(), records.len());
    let mut bodies: Vec<_> = records.iter().map(|r| r.as_bytes()[34 + 24..].to_vec()).collect();
    bodies.sort_unstable();
    bodies.dedup();
    assert_eq!(bodies.len(), records.len());
}

#[test]
fn nonces_are_not_all_zero_or_a_counter() {
    let mut source = OsNonces;
    let nonces: Vec<[u8; RECORD_NONCE_BYTES]> = (0..64).map(|_| source.next_nonce().unwrap()).collect();
    assert!(nonces.iter().all(|n| n.iter().any(|&b| b != 0)));
    // Each of the 24 byte positions takes more than a few values over 64 draws.
    for position in 0..RECORD_NONCE_BYTES {
        let mut seen: Vec<u8> = nonces.iter().map(|n| n[position]).collect();
        seen.sort_unstable();
        seen.dedup();
        assert!(seen.len() > 20, "byte {position} took only {} values in 64 draws", seen.len());
    }
}

/// Seals `count` records under one key and counts the nonces that appear twice.
#[cfg(not(target_arch = "wasm32"))]
fn repeated_nonces(count: usize) -> usize {
    let keys = keys(1);
    let mut nonces: Vec<[u8; RECORD_NONCE_BYTES]> = Vec::with_capacity(count);
    for n in 0..count {
        let record = keys.seal(header(), &(n as u64).to_le_bytes()).unwrap();
        nonces.push(*record.nonce());
    }
    nonces.sort_unstable();
    nonces.windows(2).filter(|pair| pair[0] == pair[1]).count()
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn twenty_thousand_seals_never_repeat_a_nonce() {
    assert_eq!(repeated_nonces(20_000), 0);
}

/// The million of the design (section 15). Unoptimised it takes a minute, so it is a slow test:
/// `cargo nextest run --release -p drafftink-seal --test nonces --run-ignored only`.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "a million seals: run it in a release build"]
fn a_million_seals_never_repeat_a_nonce() {
    assert_eq!(repeated_nonces(1_000_000), 0);
}
