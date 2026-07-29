use core::fmt;

use serde::{Deserialize, Serialize};

/// A proportional rate held in basis points (hundredths of a percent).
///
/// Basis points cover every rate either target market uses without touching a decimal type:
/// Bangladesh's VAT ladder includes 15%, 7.5%, 5%, 4.5% and 2.4% — that is `1500`, `750`, `500`,
/// `450`, `240` — and KSA's standard rate is 15%. The finest real-world granularity is 0.1%, so
/// 0.01% resolution leaves comfortable headroom.
///
/// Storing the rate as an integer is not a micro-optimisation. It means a rate that round-trips
/// through JSON, SQLite, and Postgres comes back bit-identical, which is a precondition for the
/// terminal and server agreeing on a total.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Rate {
    basis_points: i32,
}

impl Rate {
    /// Zero rate — exempt or zero-rated supplies.
    pub const ZERO: Self = Self { basis_points: 0 };

    /// One basis point is 0.01%.
    pub const BASIS_POINTS_PER_UNIT: i32 = 10_000;

    /// Construct from basis points: `1500` is 15%.
    #[must_use]
    pub const fn from_basis_points(basis_points: i32) -> Self {
        Self { basis_points }
    }

    /// Construct from whole percent: `15` is 15%.
    ///
    /// Returns `None` if the result would overflow — which for realistic percentages it never will,
    /// but the type refuses to hide the possibility.
    #[must_use]
    pub const fn from_percent(percent: i32) -> Option<Self> {
        match percent.checked_mul(100) {
            Some(basis_points) => Some(Self { basis_points }),
            None => None,
        }
    }

    /// The underlying basis points.
    #[must_use]
    pub const fn basis_points(self) -> i32 {
        self.basis_points
    }

    /// Whether this rate contributes nothing — true for both exempt and zero-rated supplies.
    ///
    /// Note that "exempt" and "zero-rated" are legally distinct even though the arithmetic is
    /// identical; that distinction lives in the tax classification, not here.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.basis_points == 0
    }
}

impl fmt::Display for Rate {
    /// Renders as a percentage for logs and tests only.
    ///
    /// User-facing rates are formatted by `Intl.NumberFormat` in the UI — this is deliberately not
    /// locale-aware, so it can never be mistaken for a display path.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.basis_points / 100;
        let hundredths = (self.basis_points % 100).abs();
        if hundredths == 0 {
            write!(f, "{whole}%")
        } else if hundredths % 10 == 0 {
            write!(f, "{whole}.{}%", hundredths / 10)
        } else {
            write!(f, "{whole}.{hundredths:02}%")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_constructor_matches_basis_points() {
        assert_eq!(Rate::from_percent(15), Some(Rate::from_basis_points(1500)));
        assert_eq!(Rate::from_percent(0), Some(Rate::ZERO));
    }

    #[test]
    fn displays_the_real_bangladesh_vat_ladder() {
        assert_eq!(Rate::from_basis_points(1500).to_string(), "15%");
        assert_eq!(Rate::from_basis_points(750).to_string(), "7.5%");
        assert_eq!(Rate::from_basis_points(500).to_string(), "5%");
        assert_eq!(Rate::from_basis_points(450).to_string(), "4.5%");
        assert_eq!(Rate::from_basis_points(240).to_string(), "2.4%");
    }

    #[test]
    fn zero_is_recognised() {
        assert!(Rate::ZERO.is_zero());
        assert!(!Rate::from_basis_points(1).is_zero());
    }

    #[test]
    fn survives_a_json_round_trip_unchanged() {
        // Bit-identical round-tripping is what lets terminal and server agree on a total.
        for basis_points in [0, 240, 450, 500, 750, 1500] {
            let rate = Rate::from_basis_points(basis_points);
            let encoded = serde_json::to_string(&rate).expect("serialises");
            assert_eq!(encoded, basis_points.to_string());
            let decoded: Rate = serde_json::from_str(&encoded).expect("deserialises");
            assert_eq!(decoded, rate);
        }
    }
}
