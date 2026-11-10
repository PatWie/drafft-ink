//! The old app's random generator, bit for bit (`vello_impl.rs` `SimpleRng`, xorshift32).

pub(crate) struct SimpleRng {
    state: u32,
}

impl SimpleRng {
    pub(crate) fn new(seed: u32) -> Self {
        Self { state: seed.max(1) }
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Uniform in [-1, 1].
    fn next_f64(&mut self) -> f64 {
        (f64::from(self.next_u32()) / f64::from(u32::MAX)) * 2.0 - 1.0
    }

    /// Random offset in [-amount, amount].
    pub(crate) fn offset(&mut self, amount: f64) -> f64 {
        self.next_f64() * amount
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_seed_is_remapped_and_sequence_is_stable() {
        let mut a = SimpleRng::new(0);
        let mut b = SimpleRng::new(1);
        for _ in 0..8 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
        // xorshift32 from state 1: first outputs are well known.
        let mut r = SimpleRng::new(1);
        assert_eq!(r.next_u32(), 270_369);
        assert_eq!(r.next_u32(), 67_634_689);
    }

    #[test]
    fn offsets_stay_in_range() {
        let mut r = SimpleRng::new(7);
        for _ in 0..1000 {
            let o = r.offset(3.0);
            assert!((-3.0..=3.0).contains(&o));
        }
    }
}
