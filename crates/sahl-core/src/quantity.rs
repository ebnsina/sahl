//! Quantities, held as exact thousandths of a unit.
//!
//! A POS that only counts whole units cannot sell a grocery profile. Weighed goods — 1.234 kg of
//! rice, 0.750 kg of fish — are the normal case in the market Sahl targets first, so quantity is a
//! scaled integer for exactly the same reason money is: three decimal places of milli-units, no
//! floating point anywhere.
//!
//! Three decimals is the resolution of a gram against a kilogram, which is what retail scales
//! report and what receipts print.

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::money::MoneyError;

/// An exact quantity in thousandths of a unit.
///
/// `Quantity::from_milli(1_234)` is 1.234 kg. `Quantity::from_units(3)` is 3 pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Quantity {
    milli: i64,
}

impl Quantity {
    /// Thousandths per whole unit.
    pub const MILLI_PER_UNIT: i64 = 1_000;

    /// A single unit — the overwhelmingly common case at a retail till.
    pub const ONE: Self = Self {
        milli: Self::MILLI_PER_UNIT,
    };

    /// Zero.
    pub const ZERO: Self = Self { milli: 0 };

    /// Construct from thousandths: `1_234` is 1.234.
    #[must_use]
    pub const fn from_milli(milli: i64) -> Self {
        Self { milli }
    }

    /// Construct from whole units.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] if the quantity does not fit.
    pub fn from_units(units: i64) -> Result<Self, MoneyError> {
        let milli = units
            .checked_mul(Self::MILLI_PER_UNIT)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self { milli })
    }

    /// The exact count of thousandths.
    #[must_use]
    pub const fn milli(self) -> i64 {
        self.milli
    }

    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.milli == 0
    }

    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.milli < 0
    }

    /// Add two quantities.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] on `i64` overflow.
    pub fn checked_add(self, rhs: Self) -> Result<Self, MoneyError> {
        let milli = self
            .milli
            .checked_add(rhs.milli)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self { milli })
    }

    /// Negate — a return line is a negative quantity.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] at `i64::MIN`.
    pub fn checked_neg(self) -> Result<Self, MoneyError> {
        let milli = self.milli.checked_neg().ok_or(MoneyError::Overflow)?;
        Ok(Self { milli })
    }
}

impl fmt::Display for Quantity {
    /// For logs and tests only — user-facing quantities are formatted by `Intl.NumberFormat`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Some(units), Some(remainder)) = (
            self.milli.checked_div(Self::MILLI_PER_UNIT),
            self.milli.checked_rem(Self::MILLI_PER_UNIT),
        ) else {
            return f.write_str("<invalid quantity>");
        };
        let fraction = remainder.unsigned_abs();
        let sign = if self.milli < 0 && units == 0 {
            "-"
        } else {
            ""
        };
        write!(f, "{sign}{units}.{fraction:03}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_units_scale_to_thousandths() {
        assert_eq!(Quantity::from_units(3), Ok(Quantity::from_milli(3_000)));
        assert_eq!(Quantity::ONE, Quantity::from_milli(1_000));
    }

    #[test]
    fn represents_a_weighed_grocery_line() {
        // 1.234 kg off a scale.
        let weighed = Quantity::from_milli(1_234);
        assert_eq!(weighed.to_string(), "1.234");
    }

    #[test]
    fn negative_quantities_display_correctly() {
        assert_eq!(Quantity::from_milli(-1_234).to_string(), "-1.234");
        assert_eq!(Quantity::from_milli(-750).to_string(), "-0.750");
    }

    #[test]
    fn addition_is_checked() {
        assert_eq!(
            Quantity::from_milli(i64::MAX).checked_add(Quantity::ONE),
            Err(MoneyError::Overflow)
        );
    }
}
