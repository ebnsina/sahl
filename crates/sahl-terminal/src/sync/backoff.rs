//! How long to wait between sync attempts.
//!
//! Pure and separate so the schedule can be tested without waiting for it.

use std::time::Duration;

/// Exponential backoff with jitter.
///
/// The jitter is not decoration. Power and internet outages in the target market take out whole
/// neighbourhoods, so tills come back together — and a fleet retrying on identical schedules would
/// hit the server in synchronised waves, each wave making the next one more likely. Spreading the
/// retries is what stops a recovery turning into a self-inflicted outage.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
    /// Jitter is drawn from this, advanced by a small LCG rather than a real RNG: the till needs
    /// spread, not unpredictability, and this keeps the module free of a dependency and testable.
    seed: u64,
}

impl Backoff {
    /// Delay between successful rounds when there is nothing pending.
    pub const IDLE: Duration = Duration::from_secs(30);

    #[must_use]
    pub const fn new(base: Duration, max: Duration, seed: u64) -> Self {
        Self {
            base,
            max,
            attempt: 0,
            seed,
        }
    }

    /// Sensible defaults: first retry in a second, capped at five minutes.
    ///
    /// Capped rather than unbounded because a till that has backed off to an hour looks broken to a
    /// merchant, and the sales it is holding are the ones nobody else has a copy of.
    #[must_use]
    pub const fn standard(seed: u64) -> Self {
        Self::new(Duration::from_secs(1), Duration::from_secs(300), seed)
    }

    /// A round succeeded — go back to normal pace.
    pub const fn reset(&mut self) {
        self.attempt = 0;
    }

    #[must_use]
    pub const fn attempts(&self) -> u32 {
        self.attempt
    }

    /// How long to wait before the next attempt, doubling each failure.
    pub fn next_delay(&mut self) -> Duration {
        // Saturating shift: after ~30 failures this pins at the cap rather than wrapping to zero and
        // hammering the server exactly when it is least able to cope.
        let factor = 1u64.checked_shl(self.attempt.min(30)).unwrap_or(u64::MAX);
        let raw = self
            .base
            .saturating_mul(u32::try_from(factor).unwrap_or(u32::MAX));
        let capped = raw.min(self.max);

        self.attempt = self.attempt.saturating_add(1);

        // Full jitter over [capped/2, capped]. Half the wait is kept so backoff still means backoff;
        // the rest is spread so a neighbourhood of tills does not retry in lockstep.
        let half = capped.checked_div(2).unwrap_or(Duration::ZERO);
        let spread = capped.saturating_sub(half);
        half.saturating_add(self.jitter(spread))
    }

    fn jitter(&mut self, span: Duration) -> Duration {
        if span.is_zero() {
            return Duration::ZERO;
        }
        // xorshift64 — cheap, deterministic given a seed, and adequate for spreading retries.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;

        let span_millis = u64::try_from(span.as_millis()).unwrap_or(u64::MAX).max(1);
        Duration::from_millis(self.seed.checked_rem(span_millis).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standard() -> Backoff {
        Backoff::standard(0x2545_F491_4F6C_DD1D)
    }

    #[test]
    fn the_first_delay_is_around_the_base() {
        let mut backoff = standard();
        let delay = backoff.next_delay();
        assert!(
            delay >= Duration::from_millis(500),
            "at least half the base"
        );
        assert!(delay <= Duration::from_secs(1));
    }

    #[test]
    fn delays_grow_and_then_stop_at_the_cap() {
        // Capped because a till backed off to an hour looks broken, and it is holding the only copy
        // of those sales.
        let mut backoff = standard();
        for _ in 0..40 {
            let delay = backoff.next_delay();
            assert!(delay <= Duration::from_secs(300), "never exceeds the cap");
        }
        assert!(
            backoff.next_delay() >= Duration::from_secs(150),
            "and settles at the cap, not at zero"
        );
    }

    #[test]
    fn a_long_run_of_failures_does_not_wrap_to_zero() {
        // The shift would overflow around attempt 64 and silently produce no delay — hammering the
        // server exactly when it is least able to cope.
        let mut backoff = standard();
        for _ in 0..200 {
            backoff.next_delay();
        }
        assert!(backoff.next_delay() >= Duration::from_secs(150));
    }

    #[test]
    fn success_returns_to_the_base_delay() {
        let mut backoff = standard();
        for _ in 0..10 {
            backoff.next_delay();
        }
        backoff.reset();

        assert_eq!(backoff.attempts(), 0);
        assert!(backoff.next_delay() <= Duration::from_secs(1));
    }

    #[test]
    fn two_tills_recovering_together_do_not_retry_in_lockstep() {
        // The scenario this exists for: an area outage ends and every till reconnects at once.
        let mut first = Backoff::standard(1);
        let mut second = Backoff::standard(999_331);

        for _ in 0..5 {
            first.next_delay();
            second.next_delay();
        }

        assert_ne!(
            first.next_delay(),
            second.next_delay(),
            "different seeds must diverge"
        );
    }

    #[test]
    fn jitter_never_removes_more_than_half_the_wait() {
        // Backoff still has to mean backoff.
        //
        // Warm past the cap first: with a 1s base and 300s cap, doubling only reaches the ceiling
        // around attempt 9, so a shorter warm-up would be testing an uncapped delay.
        let mut backoff = standard();
        for _ in 0..12 {
            backoff.next_delay();
        }
        for _ in 0..20 {
            let delay = backoff.next_delay();
            assert!(
                delay >= Duration::from_secs(150),
                "at least half of the 300s cap"
            );
        }
    }

    #[test]
    fn the_same_seed_replays_the_same_schedule() {
        // Determinism is what makes this testable at all.
        let mut first = Backoff::standard(42);
        let mut second = Backoff::standard(42);
        for _ in 0..10 {
            assert_eq!(first.next_delay(), second.next_delay());
        }
    }
}
