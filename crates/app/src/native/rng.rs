//! Operating-system randomness for ids and seeds.

use drafftink_scene::Rng64;

/// 64 random bits per call from the operating system. If the system refuses (it practically never does), the
/// bits come from the clock, which is still enough to keep ids from colliding within one session.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct OsRng;

impl Rng64 for OsRng {
    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        if getrandom::fill(&mut bytes).is_err() {
            let nanos = web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(0));
            return nanos ^ 0x9E37_79B9_7F4A_7C15;
        }
        u64::from_le_bytes(bytes)
    }
}

/// 128 random bits.
pub(crate) fn random_u128() -> u128 {
    let mut rng = OsRng;
    (u128::from(rng.next_u64()) << 64) | u128::from(rng.next_u64())
}
