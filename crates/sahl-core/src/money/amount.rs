use core::fmt;

use serde::{Deserialize, Serialize};

use super::currency::Currency;
use super::error::MoneyError;
use super::rate::Rate;
use super::rounding::{Rounding, divide_rounded};

/// A monetary amount, held as an exact count of minor units.
///
/// `Money { minor: 12_345, currency: Bdt }` is ৳123.45. There is no decimal representation anywhere
/// in this type, because there is no rounding error anywhere in this type.
///
/// Arithmetic is checked and returns `Result`. That is deliberately noisier than operator overloads
/// would be: overflow and currency mismatch are conditions a POS must surface, not absorb. `Add`
/// and `Sub` are intentionally *not* implemented — there is no correct panicking version of adding
/// two amounts that might be in different currencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Money {
    minor: i64,
    currency: Currency,
}

impl Money {
    /// Construct from an exact count of minor units.
    #[must_use]
    pub const fn from_minor(minor: i64, currency: Currency) -> Self {
        Self { minor, currency }
    }

    /// Zero in the given currency.
    #[must_use]
    pub const fn zero(currency: Currency) -> Self {
        Self { minor: 0, currency }
    }

    /// Construct from whole major units — ৳50 is `from_major(50, Bdt)`.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] if the amount does not fit in `i64` minor units.
    pub fn from_major(major: i64, currency: Currency) -> Result<Self, MoneyError> {
        let minor = major
            .checked_mul(currency.minor_per_major())
            .ok_or(MoneyError::Overflow)?;
        Ok(Self { minor, currency })
    }

    /// The exact count of minor units.
    #[must_use]
    pub const fn minor(self) -> i64 {
        self.minor
    }

    /// The currency this amount is denominated in.
    #[must_use]
    pub const fn currency(self) -> Currency {
        self.currency
    }

    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.minor == 0
    }

    #[must_use]
    pub const fn is_positive(self) -> bool {
        self.minor > 0
    }

    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.minor < 0
    }

    /// Add two amounts of the same currency.
    ///
    /// # Errors
    /// [`MoneyError::CurrencyMismatch`] if the currencies differ, [`MoneyError::Overflow`] on
    /// `i64` overflow.
    pub fn checked_add(self, rhs: Self) -> Result<Self, MoneyError> {
        self.assert_same_currency(rhs)?;
        let minor = self
            .minor
            .checked_add(rhs.minor)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Subtract an amount of the same currency.
    ///
    /// # Errors
    /// [`MoneyError::CurrencyMismatch`] if the currencies differ, [`MoneyError::Overflow`] on
    /// `i64` overflow.
    pub fn checked_sub(self, rhs: Self) -> Result<Self, MoneyError> {
        self.assert_same_currency(rhs)?;
        let minor = self
            .minor
            .checked_sub(rhs.minor)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Multiply by a whole count — a unit price by a quantity, say.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] on `i64` overflow.
    pub fn checked_mul(self, factor: i64) -> Result<Self, MoneyError> {
        let minor = self.minor.checked_mul(factor).ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Negate — the sale-to-refund mirror.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] at `i64::MIN`, which has no positive counterpart.
    pub fn checked_neg(self) -> Result<Self, MoneyError> {
        let minor = self.minor.checked_neg().ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Absolute value.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] at `i64::MIN`.
    pub fn checked_abs(self) -> Result<Self, MoneyError> {
        let minor = self.minor.checked_abs().ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Sum an iterator of amounts, all of which must share `currency`.
    ///
    /// Takes the currency explicitly so that summing an empty basket yields a correctly denominated
    /// zero rather than failing or guessing.
    ///
    /// # Errors
    /// [`MoneyError::CurrencyMismatch`] or [`MoneyError::Overflow`].
    pub fn try_sum<I>(amounts: I, currency: Currency) -> Result<Self, MoneyError>
    where
        I: IntoIterator<Item = Self>,
    {
        amounts
            .into_iter()
            .try_fold(Self::zero(currency), Self::checked_add)
    }

    /// Scale by the exact ratio `numerator / denominator`, resolving the fraction per `rounding`.
    ///
    /// This is the single primitive every proportional money operation is built from — tax,
    /// percentage discounts, service charge, tip apportionment. The intermediate product is `i128`,
    /// because `price × quantity × basis_points` overflows `i64` on baskets that are large but
    /// entirely realistic in a wholesale or grocery setting.
    ///
    /// # Errors
    /// [`MoneyError::DivisionByZero`] if `denominator` is zero, [`MoneyError::Overflow`] if the
    /// result leaves `i64` range.
    pub fn mul_ratio(
        self,
        numerator: i64,
        denominator: i64,
        rounding: Rounding,
    ) -> Result<Self, MoneyError> {
        let product = i128::from(self.minor)
            .checked_mul(i128::from(numerator))
            .ok_or(MoneyError::Overflow)?;
        let scaled = divide_rounded(product, i128::from(denominator), rounding)?;
        let minor = i64::try_from(scaled).map_err(|_| MoneyError::Overflow)?;
        Ok(Self {
            minor,
            currency: self.currency,
        })
    }

    /// Apply a proportional rate — 15% of this amount.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] if the result leaves `i64` range.
    pub fn apply_rate(self, rate: Rate, rounding: Rounding) -> Result<Self, MoneyError> {
        self.mul_ratio(
            i64::from(rate.basis_points()),
            i64::from(Rate::BASIS_POINTS_PER_UNIT),
            rounding,
        )
    }

    /// Divide this amount across `weights`, preserving the total exactly.
    ///
    /// Uses largest-remainder apportionment: every part gets its truncated share, then the leftover
    /// minor units go one each to the parts with the largest discarded fractions, ties broken by
    /// position. **The returned amounts always sum to exactly `self`** — that is the entire reason
    /// this function exists rather than callers dividing and rounding.
    ///
    /// This is what splits a bill three ways, apportions an order-level discount across lines so
    /// per-line VAT stays correct, and distributes a rounding adjustment. Naive division loses cents
    /// on all three, and a till that is short by one taka every few hundred transactions is a
    /// support burden that never ends.
    ///
    /// Tie-breaking is positional and therefore deterministic, which matters more than it looks:
    /// the terminal and the server both compute this, and they must agree byte for byte.
    ///
    /// # Errors
    /// [`MoneyError::InvalidWeights`] if `weights` is empty or sums to zero,
    /// [`MoneyError::Overflow`] on intermediate overflow.
    pub fn allocate(self, weights: &[u64]) -> Result<Vec<Self>, MoneyError> {
        if weights.is_empty() {
            return Err(MoneyError::InvalidWeights);
        }

        let total_weight = weights
            .iter()
            .try_fold(0u128, |acc, weight| acc.checked_add(u128::from(*weight)))
            .ok_or(MoneyError::Overflow)?;
        if total_weight == 0 {
            return Err(MoneyError::InvalidWeights);
        }

        // Work on the magnitude and reapply the sign at the end. `unsigned_abs` is used rather than
        // `checked_abs` so that `i64::MIN` allocates correctly instead of erroring.
        let negative = self.minor < 0;
        let magnitude = u128::from(self.minor.unsigned_abs());

        let mut shares: Vec<u128> = Vec::with_capacity(weights.len());
        let mut remainders: Vec<u128> = Vec::with_capacity(weights.len());
        for weight in weights {
            let product = magnitude
                .checked_mul(u128::from(*weight))
                .ok_or(MoneyError::Overflow)?;
            shares.push(
                product
                    .checked_div(total_weight)
                    .ok_or(MoneyError::DivisionByZero)?,
            );
            remainders.push(
                product
                    .checked_rem(total_weight)
                    .ok_or(MoneyError::DivisionByZero)?,
            );
        }

        let distributed = shares
            .iter()
            .try_fold(0u128, |acc, share| acc.checked_add(*share))
            .ok_or(MoneyError::Overflow)?;
        let leftover = magnitude
            .checked_sub(distributed)
            .ok_or(MoneyError::Overflow)?;

        // `leftover` is strictly less than the number of parts, so this conversion cannot saturate
        // in any real case; `try_from` keeps it total regardless.
        let leftover_parts = usize::try_from(leftover).unwrap_or(usize::MAX);
        if leftover_parts > 0 {
            let mut order: Vec<usize> = (0..shares.len()).collect();
            order.sort_by(|left, right| {
                let left_remainder = remainders.get(*left).copied().unwrap_or(0);
                let right_remainder = remainders.get(*right).copied().unwrap_or(0);
                right_remainder.cmp(&left_remainder).then(left.cmp(right))
            });
            for index in order.into_iter().take(leftover_parts) {
                if let Some(share) = shares.get_mut(index) {
                    *share = share.checked_add(1).ok_or(MoneyError::Overflow)?;
                }
            }
        }

        shares
            .into_iter()
            .map(|share| {
                let value = i64::try_from(share).map_err(|_| MoneyError::Overflow)?;
                let minor = if negative {
                    value.checked_neg().ok_or(MoneyError::Overflow)?
                } else {
                    value
                };
                Ok(Self {
                    minor,
                    currency: self.currency,
                })
            })
            .collect()
    }

    /// Split evenly into `parts`, preserving the total exactly.
    ///
    /// Splitting ৳10.00 three ways gives 3.34, 3.33, 3.33 — not three amounts that sum to 9.99.
    ///
    /// # Errors
    /// [`MoneyError::InvalidWeights`] if `parts` is zero.
    pub fn split(self, parts: usize) -> Result<Vec<Self>, MoneyError> {
        if parts == 0 {
            return Err(MoneyError::InvalidWeights);
        }
        self.allocate(&vec![1u64; parts])
    }

    fn assert_same_currency(self, rhs: Self) -> Result<(), MoneyError> {
        if self.currency == rhs.currency {
            Ok(())
        } else {
            Err(MoneyError::CurrencyMismatch {
                left: self.currency,
                right: rhs.currency,
            })
        }
    }
}

impl fmt::Display for Money {
    /// Renders as `123.45 BDT`, for logs, tests, and error messages **only**.
    ///
    /// This is deliberately not locale-aware and deliberately ugly. User-facing money is formatted
    /// by `Intl.NumberFormat` in the UI layer; if this output ever reaches a receipt, that is a bug
    /// and it should look like one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let per_major = self.currency.minor_per_major();
        let (Some(major), Some(remainder)) = (
            self.minor.checked_div(per_major),
            self.minor.checked_rem(per_major),
        ) else {
            // Unreachable while `minor_per_major` stays non-zero, but this is a `Display` impl:
            // it must not panic even if that invariant is someday broken.
            return f.write_str("<invalid amount>");
        };
        let minor = remainder.unsigned_abs();
        let sign = if self.minor < 0 && major == 0 {
            "-"
        } else {
            ""
        };
        let width = usize::from(self.currency.exponent());
        write!(
            f,
            "{sign}{major}.{minor:0width$} {}",
            self.currency,
            width = width
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BDT: Currency = Currency::Bdt;
    const SAR: Currency = Currency::Sar;

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, BDT)
    }

    #[test]
    fn major_units_convert_to_minor() {
        assert_eq!(Money::from_major(50, BDT), Ok(bdt(5_000)));
    }

    #[test]
    fn addition_and_subtraction_are_exact() {
        assert_eq!(bdt(1_050).checked_add(bdt(2_575)), Ok(bdt(3_625)));
        assert_eq!(bdt(1_050).checked_sub(bdt(2_575)), Ok(bdt(-1_525)));
    }

    #[test]
    fn mixing_currencies_is_refused_not_coerced() {
        let result = bdt(100).checked_add(Money::from_minor(100, SAR));
        assert_eq!(
            result,
            Err(MoneyError::CurrencyMismatch {
                left: BDT,
                right: SAR
            })
        );
    }

    #[test]
    fn overflow_is_reported_rather_than_wrapped() {
        assert_eq!(
            Money::from_minor(i64::MAX, BDT).checked_add(bdt(1)),
            Err(MoneyError::Overflow)
        );
        assert_eq!(
            Money::from_minor(i64::MIN, BDT).checked_neg(),
            Err(MoneyError::Overflow)
        );
    }

    #[test]
    fn summing_an_empty_basket_yields_a_denominated_zero() {
        assert_eq!(Money::try_sum([], BDT), Ok(Money::zero(BDT)));
    }

    #[test]
    fn applies_the_standard_vat_rates() {
        // ৳100.00 at 15% is ৳15.00 in both target markets.
        let rate = Rate::from_basis_points(1500);
        assert_eq!(
            bdt(10_000).apply_rate(rate, Rounding::HalfUp),
            Ok(bdt(1_500))
        );
        // 7.5% of ৳33.33 is 2.49975 → 2.50 half-up.
        let reduced = Rate::from_basis_points(750);
        assert_eq!(
            bdt(3_333).apply_rate(reduced, Rounding::HalfUp),
            Ok(bdt(250))
        );
    }

    #[test]
    fn large_baskets_do_not_overflow_the_intermediate() {
        // A wholesale line big enough that `minor * basis_points` leaves i64 range. This is the
        // case that silently corrupts totals if the intermediate is not widened.
        let big = bdt(1_000_000_000_000_000);
        let rate = Rate::from_basis_points(1500);
        assert_eq!(
            big.apply_rate(rate, Rounding::HalfUp),
            Ok(bdt(150_000_000_000_000))
        );
    }

    #[test]
    fn splitting_never_loses_a_minor_unit() {
        let parts = bdt(1_000).split(3).expect("splits");
        assert_eq!(parts, vec![bdt(334), bdt(333), bdt(333)]);
        assert_eq!(Money::try_sum(parts, BDT), Ok(bdt(1_000)));
    }

    #[test]
    fn allocation_follows_weights_and_still_sums_exactly() {
        // The classic: 5 cents across 3:7 must not become 1.5 and 3.5.
        let parts = bdt(5).allocate(&[3, 7]).expect("allocates");
        assert_eq!(Money::try_sum(parts.clone(), BDT), Ok(bdt(5)));
        assert_eq!(parts, vec![bdt(2), bdt(3)]);
    }

    #[test]
    fn allocation_is_sign_symmetric() {
        // A refund must mirror its sale exactly, line for line.
        let sale = bdt(1_000).allocate(&[1, 1, 1]).expect("allocates");
        let refund = bdt(-1_000).allocate(&[1, 1, 1]).expect("allocates");
        for (sold, refunded) in sale.iter().zip(refund.iter()) {
            assert_eq!(sold.checked_neg(), Ok(*refunded));
        }
    }

    #[test]
    fn allocation_rejects_degenerate_weights() {
        assert_eq!(bdt(100).allocate(&[]), Err(MoneyError::InvalidWeights));
        assert_eq!(bdt(100).allocate(&[0, 0]), Err(MoneyError::InvalidWeights));
        assert_eq!(bdt(100).split(0), Err(MoneyError::InvalidWeights));
    }

    #[test]
    fn allocation_handles_the_i64_min_edge() {
        // `abs()` would overflow here; `unsigned_abs` is why this works.
        let parts = Money::from_minor(i64::MIN, BDT).split(2).expect("splits");
        assert_eq!(
            Money::try_sum(parts, BDT),
            Ok(Money::from_minor(i64::MIN, BDT))
        );
    }

    #[test]
    fn display_is_for_logs_and_shows_it() {
        assert_eq!(bdt(12_345).to_string(), "123.45 BDT");
        assert_eq!(bdt(-12_345).to_string(), "-123.45 BDT");
        assert_eq!(bdt(-45).to_string(), "-0.45 BDT");
        assert_eq!(bdt(0).to_string(), "0.00 BDT");
    }
}
