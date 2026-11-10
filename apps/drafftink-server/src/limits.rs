//! Rate limiting: a token bucket that takes the time as an argument, so tests need no clock.

use std::time::Duration;

use tokio::time::Instant;

/// A token bucket. `rate` tokens per second refill it up to `burst` tokens.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    /// A full bucket. A rate of 0 is treated as 1 so a bucket never blocks for ever.
    pub fn new(rate_per_sec: u32, burst: u32, now: Instant) -> Self {
        let burst = f64::from(burst.max(1));
        TokenBucket { rate: f64::from(rate_per_sec.max(1)), burst, tokens: burst, last: now }
    }

    /// Takes one token, or says how long to wait until one is available.
    pub fn take(&mut self, now: Instant) -> Result<(), Duration> {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - self.tokens) / self.rate))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_the_burst_then_refuses_then_refills() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(10, 20, t0);
        for i in 0..20 {
            assert!(b.take(t0).is_ok(), "token {i}");
        }
        let wait = b.take(t0).expect_err("bucket is empty");
        assert!(wait <= Duration::from_millis(100) && wait > Duration::ZERO, "{wait:?}");
        assert!(b.take(t0 + Duration::from_millis(100)).is_ok());
        assert!(b.take(t0 + Duration::from_millis(100)).is_err());
    }

    #[test]
    fn refill_is_capped_at_the_burst() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(10, 5, t0);
        let later = t0 + Duration::from_secs(3600);
        for _ in 0..5 {
            assert!(b.take(later).is_ok());
        }
        assert!(b.take(later).is_err());
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_panic_or_refill() {
        let t0 = Instant::now() + Duration::from_secs(10);
        let mut b = TokenBucket::new(1, 1, t0);
        assert!(b.take(t0).is_ok());
        assert!(b.take(t0 - Duration::from_secs(5)).is_err());
    }
}
