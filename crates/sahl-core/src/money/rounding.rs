use serde::{Deserialize, Serialize};

use super::error::MoneyError;

/// How to resolve a fraction of a minor unit.
///
/// Jurisdiction matters here: VAT calculation in both Bangladesh and the Gulf conventionally rounds
/// half away from zero, so [`Rounding::HalfUp`] is the default for tax. [`Rounding::HalfEven`] is
/// kept for statistical aggregation, where repeated half-up rounding introduces an upward drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    /// Half away from zero: 2.5 → 3, −2.5 → −3. The tax default.
    #[default]
    HalfUp,
    /// Half to even ("banker's"): 2.5 → 2, 3.5 → 4. Avoids drift when summing many rounded values.
    HalfEven,
    /// Toward zero, always. Used where a merchant must never be over-credited.
    TowardZero,
}

/// Divide `numerator` by `denominator`, resolving the fraction per `mode`.
///
/// Works in `i128` because callers reach this after multiplying two `i64`-scale quantities; doing
/// the intermediate in `i64` is exactly the overflow that quietly corrupts large baskets.
///
/// # Errors
/// [`MoneyError::DivisionByZero`] if `denominator` is zero, [`MoneyError::Overflow`] if the
/// rounding adjustment leaves `i128` range.
pub(crate) fn divide_rounded(
    numerator: i128,
    denominator: i128,
    mode: Rounding,
) -> Result<i128, MoneyError> {
    if denominator == 0 {
        return Err(MoneyError::DivisionByZero);
    }

    let quotient = numerator
        .checked_div(denominator)
        .ok_or(MoneyError::Overflow)?;
    let remainder = numerator
        .checked_rem(denominator)
        .ok_or(MoneyError::Overflow)?;

    if remainder == 0 {
        return Ok(quotient);
    }

    // Rust's `/` truncates toward zero, so the discarded fraction always has the sign of the true
    // result. `step` is the direction we would move to round away from zero.
    let step: i128 = if (numerator < 0) == (denominator < 0) {
        1
    } else {
        -1
    };

    let twice_remainder = remainder
        .checked_abs()
        .and_then(|r| r.checked_mul(2))
        .ok_or(MoneyError::Overflow)?;
    let magnitude = denominator.checked_abs().ok_or(MoneyError::Overflow)?;

    let round_away = match mode {
        Rounding::TowardZero => false,
        Rounding::HalfUp => twice_remainder >= magnitude,
        Rounding::HalfEven => match twice_remainder.cmp(&magnitude) {
            core::cmp::Ordering::Greater => true,
            core::cmp::Ordering::Less => false,
            // Exactly half: move only if it would otherwise land on an odd number.
            core::cmp::Ordering::Equal => {
                quotient.checked_rem(2).ok_or(MoneyError::Overflow)?.abs() == 1
            }
        },
    };

    if round_away {
        quotient.checked_add(step).ok_or(MoneyError::Overflow)
    } else {
        Ok(quotient)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn half_up(numerator: i128, denominator: i128) -> i128 {
        divide_rounded(numerator, denominator, Rounding::HalfUp).expect("valid division")
    }

    fn half_even(numerator: i128, denominator: i128) -> i128 {
        divide_rounded(numerator, denominator, Rounding::HalfEven).expect("valid division")
    }

    #[test]
    fn exact_division_is_untouched() {
        assert_eq!(half_up(100, 4), 25);
        assert_eq!(half_up(-100, 4), -25);
    }

    #[test]
    fn half_up_rounds_away_from_zero_symmetrically() {
        // The symmetry matters: a refund must mirror its sale exactly, or returns leak money.
        assert_eq!(half_up(5, 2), 3);
        assert_eq!(half_up(-5, 2), -3);
        assert_eq!(half_up(7, 2), 4);
        assert_eq!(half_up(-7, 2), -4);
    }

    #[test]
    fn half_up_leaves_sub_half_fractions_alone() {
        assert_eq!(half_up(4, 3), 1);
        assert_eq!(half_up(-4, 3), -1);
    }

    #[test]
    fn half_even_breaks_ties_toward_even() {
        assert_eq!(half_even(5, 2), 2);
        assert_eq!(half_even(7, 2), 4);
        assert_eq!(half_even(-5, 2), -2);
        assert_eq!(half_even(-7, 2), -4);
    }

    #[test]
    fn half_even_is_not_a_tie_breaker_when_there_is_no_tie() {
        assert_eq!(half_even(8, 3), 3);
        assert_eq!(half_even(-8, 3), -3);
    }

    #[test]
    fn toward_zero_never_grows_the_magnitude() {
        assert_eq!(
            divide_rounded(9, 2, Rounding::TowardZero).expect("valid"),
            4
        );
        assert_eq!(
            divide_rounded(-9, 2, Rounding::TowardZero).expect("valid"),
            -4
        );
    }

    #[test]
    fn negative_denominators_behave_like_their_positive_mirror() {
        assert_eq!(half_up(5, -2), -3);
        assert_eq!(half_up(-5, -2), 3);
    }

    #[test]
    fn zero_denominator_is_an_error_not_a_panic() {
        assert_eq!(
            divide_rounded(1, 0, Rounding::HalfUp),
            Err(MoneyError::DivisionByZero)
        );
    }
}
