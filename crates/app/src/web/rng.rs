//! Randomness from the browser.

use drafftink_scene::Rng64;

/// 64 random bits from `crypto.getRandomValues`. If the browser has no `crypto` (it always has, in a secure
/// context), `Math.random` fills in, which is fine for ids and not meant for secrets.
pub(crate) fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    let filled = web_sys::window()
        .and_then(|w| w.crypto().ok())
        .is_some_and(|c| c.get_random_values_with_u8_array(&mut bytes).is_ok());
    if filled {
        return u64::from_le_bytes(bytes);
    }
    // 2 x 32 bits from the 53 a double carries.
    let half = || (js_sys::Math::random() * 4_294_967_296.0) as u64;
    (half() << 32) | half()
}

/// The generator behind the element ids.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CryptoRng;

impl Rng64 for CryptoRng {
    fn next_u64(&mut self) -> u64 {
        random_u64()
    }
}
