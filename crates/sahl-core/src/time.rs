//! Timestamps, held as milliseconds since the Unix epoch.
//!
//! This is a plain integer rather than a formatted datetime for one reason: these values get
//! hashed. A timestamp that can render as `2026-07-29T10:00:00Z` or `2026-07-29T10:00:00.000Z`
//! depending on library version would produce two different event hashes for the same event, which
//! would break the chain between a terminal and the server.
//!
//! Keeping it an integer also keeps `sahl-core` free of a datetime dependency and free of a clock.
//! Nothing here reads the current time — callers supply it. That is what makes event construction a
//! pure function, and therefore replayable and testable.

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::money::MoneyError;

/// A point in time, as milliseconds since the Unix epoch (UTC).
///
/// Millisecond resolution is chosen over microseconds because it is what every platform in the
/// stack agrees on without conversion — SQLite, Postgres, and JavaScript's `Date` all round-trip
/// milliseconds exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp {
    millis: i64,
}

impl Timestamp {
    /// The Unix epoch.
    pub const EPOCH: Self = Self { millis: 0 };

    /// Construct from milliseconds since the Unix epoch.
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self { millis }
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn millis(self) -> i64 {
        self.millis
    }

    /// Whole seconds since the Unix epoch, rounding toward negative infinity.
    #[must_use]
    pub const fn as_seconds(self) -> i64 {
        self.millis.div_euclid(1_000)
    }

    /// Advance by a number of milliseconds.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] on `i64` overflow.
    pub fn checked_add_millis(self, millis: i64) -> Result<Self, MoneyError> {
        let sum = self
            .millis
            .checked_add(millis)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self { millis: sum })
    }
}

impl fmt::Display for Timestamp {
    /// Raw milliseconds, for logs and tests. User-facing times are rendered by
    /// `Intl.DateTimeFormat` in the UI, in the outlet's timezone.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_floor_toward_negative_infinity() {
        assert_eq!(Timestamp::from_millis(1_500).as_seconds(), 1);
        assert_eq!(Timestamp::from_millis(-1_500).as_seconds(), -2);
        assert_eq!(Timestamp::EPOCH.as_seconds(), 0);
    }

    #[test]
    fn serialises_as_a_bare_integer() {
        // Bare integers hash identically everywhere; formatted datetimes do not.
        let encoded =
            serde_json::to_string(&Timestamp::from_millis(1_753_000_000_000)).expect("serialises");
        assert_eq!(encoded, "1753000000000");
    }

    #[test]
    fn advancing_is_checked() {
        assert_eq!(
            Timestamp::from_millis(i64::MAX).checked_add_millis(1),
            Err(MoneyError::Overflow)
        );
    }
}
