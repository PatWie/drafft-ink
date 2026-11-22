#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Helpers shared by the test files of this crate.

use drafftink_protocol::ClientId;
use drafftink_protocol::RECORD_NONCE_BYTES;
use drafftink_seal::{NonceSource, RandomError, RoomKey, RoomKeys};

pub fn unhex(text: &str) -> Vec<u8> {
    let digit = |c: u8| char::from(c).to_digit(16).unwrap() as u8;
    text.as_bytes().chunks(2).map(|p| digit(p[0]) << 4 | digit(p[1])).collect()
}

pub fn unhex_array<const N: usize>(text: &str) -> [u8; N] {
    unhex(text).try_into().unwrap()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The room key of the vectors: the bytes 0, 1, 2, ... 31.
pub fn vector_key() -> RoomKey {
    RoomKey::from_bytes(std::array::from_fn(|i| i as u8))
}

pub fn client(n: u8) -> ClientId {
    ClientId::from_bytes([n; 16])
}

pub fn keys(seed: u8) -> RoomKeys {
    RoomKeys::new(&RoomKey::from_bytes([seed; 32]))
}

/// A nonce source that returns one fixed nonce, for known-answer tests.
pub struct FixedNonce(pub [u8; RECORD_NONCE_BYTES]);

impl NonceSource for FixedNonce {
    fn next_nonce(&mut self) -> Result<[u8; RECORD_NONCE_BYTES], RandomError> {
        Ok(self.0)
    }
}

/// Small deterministic generator for the fuzz-style tests (xorshift64*), as in `crates/protocol/tests/common`.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % (n as u64).max(1)) as usize
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}
