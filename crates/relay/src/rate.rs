//! A token bucket that takes the time as an argument.

/// `rate` tokens per second refill the bucket up to `burst` tokens.
#[derive(Debug, Clone)]
pub(crate) struct TokenBucket {
    rate_per_sec: f64,
    burst: f64,
    tokens: f64,
    last_ms: u64,
}

impl TokenBucket {
    /// A full bucket. A rate or burst of 0 counts as 1.
    pub(crate) fn new(rate_per_sec: u32, burst: u32, now_ms: u64) -> Self {
        let burst = f64::from(burst.max(1));
        TokenBucket { rate_per_sec: f64::from(rate_per_sec.max(1)), burst, tokens: burst, last_ms: now_ms }
    }

    /// Changes the refill rate, keeping the tokens that are there.
    pub(crate) fn set_rate(&mut self, rate_per_sec: u32) {
        self.rate_per_sec = f64::from(rate_per_sec.max(1));
    }

    /// Takes a token, or says in milliseconds how long to wait for one.
    pub(crate) fn take(&mut self, now_ms: u64) -> Result<(), u32> {
        let elapsed = now_ms.saturating_sub(self.last_ms) as f64 / 1000.0;
        self.last_ms = self.last_ms.max(now_ms);
        self.tokens = (self.tokens + elapsed * self.rate_per_sec).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            let wait = ((1.0 - self.tokens) / self.rate_per_sec * 1000.0).ceil();
            Err(wait.clamp(1.0, f64::from(u32::MAX)) as u32)
        }
    }
}

/// A minimum gap between two events.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MinGap {
    last_ms: Option<u64>,
}

impl MinGap {
    /// Records the event, or says in milliseconds how long to wait.
    pub(crate) fn take(&mut self, now_ms: u64, gap_ms: u64) -> Result<(), u32> {
        if let Some(last) = self.last_ms {
            let ready = last.saturating_add(gap_ms);
            if now_ms < ready {
                return Err(u32::try_from(ready - now_ms).unwrap_or(u32::MAX));
            }
        }
        self.last_ms = Some(now_ms);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_burst_is_spent_then_refilled_with_time() {
        let mut b = TokenBucket::new(10, 20, 0);
        assert!((0..20).all(|_| b.take(0).is_ok()));
        assert_eq!(b.take(0), Err(100));
        assert!(b.take(100).is_ok());
        assert!(b.take(100).is_err());
    }

    #[test]
    fn a_clock_that_goes_back_neither_refills_nor_panics() {
        let mut b = TokenBucket::new(1, 1, 10_000);
        assert!(b.take(10_000).is_ok());
        assert!(b.take(5_000).is_err());
    }

    #[test]
    fn a_minimum_gap_counts_from_the_last_accepted_event() {
        let mut gap = MinGap::default();
        assert!(gap.take(0, 1000).is_ok());
        assert_eq!(gap.take(400, 1000), Err(600));
        assert!(gap.take(1000, 1000).is_ok());
    }
}
