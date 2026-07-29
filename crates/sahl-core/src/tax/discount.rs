use serde::{Deserialize, Serialize};

use crate::money::{Money, Rate, Rounding};

use super::error::TaxError;

/// A reduction applied to a line or to a whole order.
///
/// Discounts are resolved against a base and then **clamped to that base**: a discount can take a
/// line to zero but never negative. An order-level discount larger than the order would otherwise
/// produce a negative total that the tax engine would happily compute VAT on, which is both wrong
/// and a fraud vector — a cashier who can drive a total negative can make a drawer balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Discount {
    /// No reduction.
    #[default]
    None,
    /// A proportion of the base — "10% off".
    Percentage { rate: Rate },
    /// A fixed amount off, in the order's currency.
    Amount { amount: Money },
}

impl Discount {
    /// Resolve this discount against `base`, clamped to `[0, base]`.
    ///
    /// Returns the amount to subtract, never the result of subtracting it.
    ///
    /// # Errors
    /// [`TaxError::Money`] on overflow or currency mismatch.
    pub fn resolve(self, base: Money, rounding: Rounding) -> Result<Money, TaxError> {
        // A negative base is a return line; discounts do not apply to it.
        if base.is_negative() || base.is_zero() {
            return Ok(Money::zero(base.currency()));
        }

        let raw = match self {
            Self::None => Money::zero(base.currency()),
            Self::Percentage { rate } => base.apply_rate(rate, rounding)?,
            Self::Amount { amount } => {
                if amount.currency() != base.currency() {
                    return Err(TaxError::Money(
                        crate::money::MoneyError::CurrencyMismatch {
                            left: base.currency(),
                            right: amount.currency(),
                        },
                    ));
                }
                amount
            }
        };

        if raw.is_negative() {
            return Ok(Money::zero(base.currency()));
        }
        Ok(if raw.minor() > base.minor() {
            base
        } else {
            raw
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    const BDT: Currency = Currency::Bdt;

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, BDT)
    }

    #[test]
    fn none_reduces_nothing() {
        assert_eq!(
            Discount::None.resolve(bdt(10_000), Rounding::HalfUp),
            Ok(bdt(0))
        );
    }

    #[test]
    fn percentage_is_proportional() {
        let ten_percent = Discount::Percentage {
            rate: Rate::from_basis_points(1000),
        };
        assert_eq!(
            ten_percent.resolve(bdt(10_000), Rounding::HalfUp),
            Ok(bdt(1_000))
        );
    }

    #[test]
    fn a_fixed_discount_is_capped_at_the_base() {
        // A cashier who can drive a total negative can make a drawer balance.
        let too_much = Discount::Amount {
            amount: bdt(50_000),
        };
        assert_eq!(
            too_much.resolve(bdt(10_000), Rounding::HalfUp),
            Ok(bdt(10_000))
        );
    }

    #[test]
    fn a_percentage_over_one_hundred_is_capped_too() {
        let absurd = Discount::Percentage {
            rate: Rate::from_basis_points(50_000),
        };
        assert_eq!(
            absurd.resolve(bdt(10_000), Rounding::HalfUp),
            Ok(bdt(10_000))
        );
    }

    #[test]
    fn negative_discounts_cannot_be_used_to_inflate_a_line() {
        let sneaky = Discount::Amount {
            amount: bdt(-5_000),
        };
        assert_eq!(sneaky.resolve(bdt(10_000), Rounding::HalfUp), Ok(bdt(0)));
    }

    #[test]
    fn return_lines_are_left_alone() {
        let ten_percent = Discount::Percentage {
            rate: Rate::from_basis_points(1000),
        };
        assert_eq!(
            ten_percent.resolve(bdt(-10_000), Rounding::HalfUp),
            Ok(bdt(0))
        );
    }

    #[test]
    fn a_discount_in_the_wrong_currency_is_refused() {
        let foreign = Discount::Amount {
            amount: Money::from_minor(100, Currency::Sar),
        };
        assert!(foreign.resolve(bdt(10_000), Rounding::HalfUp).is_err());
    }
}
