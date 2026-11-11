//! Hashing for cache keys: a float wrapper that hashes by value, and a 128-bit hasher.
//!
//! A cache key must change whenever an input that changes the output changes. The recipe used by
//! the caches of `sketch`, `typeset` and `render`: destructure the input without `..` (so a new
//! field is a compile error until it is hashed or deliberately ignored), wrap every float in
//! [`Bits`], and feed the result into a [`Hash128`].

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// A float that hashes by value: `-0.0` hashes like `0.0`, and every NaN hashes alike. Two floats
/// that draw the same therefore give the same key.
#[derive(Debug, Clone, Copy)]
pub struct Bits(pub f64);

impl From<f32> for Bits {
    fn from(v: f32) -> Self {
        Self(f64::from(v))
    }
}

impl Hash for Bits {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let bits = if self.0 == 0.0 {
            0
        } else if self.0.is_nan() {
            f64::NAN.to_bits()
        } else {
            self.0.to_bits()
        };
        bits.hash(state);
    }
}

/// A 128-bit hasher: two independently seeded SipHash lanes. [`Hasher::finish`] returns one lane;
/// use [`Hash128::finish128`] for the full digest. The result is stable within one process, which
/// is all an in-memory cache needs.
#[derive(Debug, Clone)]
pub struct Hash128 {
    hi: DefaultHasher,
    lo: DefaultHasher,
}

impl Default for Hash128 {
    fn default() -> Self {
        Self::new()
    }
}

impl Hash128 {
    /// An empty hasher.
    pub fn new() -> Self {
        let mut hi = DefaultHasher::new();
        let mut lo = DefaultHasher::new();
        0x5eed_u16.hash(&mut hi);
        0xfeed_u16.hash(&mut lo);
        Self { hi, lo }
    }

    /// The 128-bit digest of everything written so far.
    pub fn finish128(&self) -> u128 {
        (u128::from(self.hi.finish()) << 64) | u128::from(self.lo.finish())
    }
}

impl Hasher for Hash128 {
    fn write(&mut self, bytes: &[u8]) {
        self.hi.write(bytes);
        self.lo.write(bytes);
    }

    fn finish(&self) -> u64 {
        self.hi.finish()
    }
}

/// The 128-bit digest of `value`.
pub fn hash128<T: Hash + ?Sized>(value: &T) -> u128 {
    let mut h = Hash128::new();
    value.hash(&mut h);
    h.finish128()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_hash_by_value() {
        assert_eq!(hash128(&Bits(0.0)), hash128(&Bits(-0.0)));
        assert_eq!(hash128(&Bits(f64::NAN)), hash128(&Bits(-f64::NAN)));
        assert_ne!(hash128(&Bits(1.0)), hash128(&Bits(2.0)));
        assert_ne!(hash128(&Bits(f64::INFINITY)), hash128(&Bits(f64::NAN)));
        assert_eq!(hash128(&Bits::from(0.5_f32)), hash128(&Bits(0.5)));
    }

    #[test]
    fn the_digest_is_deterministic_and_uses_both_lanes() {
        let d = hash128("abc");
        assert_eq!(d, hash128("abc"));
        assert_ne!(d, hash128("abd"));
        assert_ne!(d >> 64, d & u128::from(u64::MAX));
    }
}
