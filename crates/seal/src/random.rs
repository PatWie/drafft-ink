//! Random bytes from the operating system (the browser's `crypto.getRandomValues` on the web).

use thiserror::Error;

use drafftink_protocol::RECORD_NONCE_BYTES;

/// The system has no source of random bytes, so nothing can be sealed and no key can be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("This device could not provide random numbers, which encryption needs ({0}).")]
pub struct RandomError(getrandom::Error);

/// Fills `buf` with random bytes from the operating system.
pub(crate) fn fill(buf: &mut [u8]) -> Result<(), RandomError> {
    getrandom::fill(buf).map_err(RandomError)
}

/// Where the nonces of sealed records come from.
///
/// [`OsNonces`] is the only source that belongs in a running program: a fresh random 24-byte nonce for every
/// record. XChaCha20-Poly1305 has a 192-bit nonce, so random nonces do not repeat in practice, and no counter has to
/// be kept across tabs, devices and restarts. Another source exists so that a test can seal with a known nonce and
/// compare the result with a known answer. A source that repeats a nonce under one key breaks the cipher.
pub trait NonceSource {
    /// The nonce of the next record.
    fn next_nonce(&mut self) -> Result<[u8; RECORD_NONCE_BYTES], RandomError>;
}

/// Nonces from the operating system's random source.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsNonces;

impl NonceSource for OsNonces {
    fn next_nonce(&mut self) -> Result<[u8; RECORD_NONCE_BYTES], RandomError> {
        let mut nonce = [0u8; RECORD_NONCE_BYTES];
        fill(&mut nonce)?;
        Ok(nonce)
    }
}
