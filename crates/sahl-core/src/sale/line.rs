use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::{Money, MoneyError};
use crate::quantity::Quantity;
use crate::tax::{Discount, LineInput, TaxClass};

/// A line on a sale.
///
/// Note what is *copied* here rather than referenced: unit price, display name, and tax class are
/// **snapshots taken at the moment of sale**, not lookups into the catalogue.
///
/// That is what makes catalogue edits safe to resolve last-writer-wins during sync. A price change
/// pushed from the back office at 3pm must not retroactively alter a receipt printed at 2pm — and
/// if lines referenced the catalogue by id, it would. It is also what lets a reprinted receipt from
/// last month still match the original, which both fiscal regimes assume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleLine {
    pub id: Uuid,
    /// The catalogue item this came from, for reporting. Never read back for pricing.
    pub product_id: Uuid,
    /// Name as printed on the receipt, snapshotted at sale time.
    pub name: String,
    /// Price per unit at the moment of sale.
    pub unit_price: Money,
    pub quantity: Quantity,
    /// VAT treatment at the moment of sale. Rates change; recorded sales do not.
    pub tax_class: TaxClass,
    pub discount: Discount,
    /// Options chosen on this line — "no ice", "extra shot", "large".
    ///
    /// Café only in practice, though nothing here refuses them elsewhere. Each carries a **per-unit**
    /// price delta, so two lattes with an extra shot each cost two shots.
    #[serde(default)]
    pub modifiers: Vec<Modifier>,
    /// A voided line is **kept, flagged, and excluded from totals** — never deleted.
    ///
    /// Deleting it would erase the evidence that the void happened, and void patterns are among the
    /// strongest fraud signals a POS has: a cashier who rings a sale, takes cash, then voids the
    /// line leaves no other trace.
    pub void: Option<LineVoid>,
}

/// Why a line was voided, and who allowed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineVoid {
    pub reason: VoidReason,
    /// The user who authorised it — a manager PIN where policy requires one. This is the field the
    /// owner-facing anomaly feed groups by.
    pub authorized_by: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum VoidReason {
    /// Rung in error.
    Mistake,
    /// Customer changed their mind before paying.
    CustomerChanged,
    /// Item damaged or expired on inspection.
    Damaged,
    /// Out of stock once picked.
    Unavailable,
}

impl SaleLine {
    /// Whether this line contributes to the totals.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.void.is_none()
    }

    /// Convert to the VAT engine's input shape.
    ///
    /// # Errors
    /// [`MoneyError`] if the options overflow or mix currencies.
    pub fn to_tax_input(&self) -> Result<LineInput, MoneyError> {
        Ok(LineInput {
            unit_price: self.effective_unit_price()?,
            quantity: self.quantity,
            tax_class: self.tax_class,
            discount: self.discount,
        })
    }

    /// What one unit of this line actually costs, options included.
    ///
    /// The deltas are per unit and are added *before* quantity is applied, which is the only reading
    /// that makes two lattes with an extra shot each cost two shots. Adding them afterwards would
    /// charge for one.
    ///
    /// A modifier's tax treatment is the line's. An extra shot in a coffee is taxed as coffee, not
    /// as some separate supply — treating it otherwise would put a second VAT class on one item and
    /// make the summary unreconcilable against the line.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow or a currency mismatch.
    pub fn effective_unit_price(&self) -> Result<Money, MoneyError> {
        let mut price = self.unit_price;
        for modifier in &self.modifiers {
            price = price.checked_add(modifier.price_delta)?;
        }
        Ok(price)
    }

    /// What this line is worth before tax and before any discount.
    ///
    /// For deciding whether an action on the line needs approval — not for printing. The order
    /// calculation owns the figures that reach a receipt, and this deliberately does not consult
    /// the discount: a threshold that fell as a discount grew would let a large void through by
    /// discounting it first.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow or a currency mismatch.
    pub fn line_value(&self) -> Result<Money, MoneyError> {
        self.effective_unit_price()?.mul_ratio(
            self.quantity.milli(),
            crate::quantity::Quantity::MILLI_PER_UNIT,
            crate::money::Rounding::HalfUp,
        )
    }

    /// What the options add to one unit.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow or a currency mismatch.
    pub fn modifier_total(&self) -> Result<Money, MoneyError> {
        let mut total = Money::from_minor(0, self.unit_price.currency());
        for modifier in &self.modifiers {
            total = total.checked_add(modifier.price_delta)?;
        }
        Ok(total)
    }
}

/// An option chosen on a line.
///
/// Snapshotted like everything else on a line: the name and the delta are recorded as they were, so
/// a catalogue edit at 3pm cannot change what a receipt printed at 2pm said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifier {
    /// The catalogue option this came from, for reporting. Never read back for pricing.
    pub option_id: Uuid,
    /// As printed on the receipt and the kitchen ticket.
    pub name: String,
    /// What it adds to **one unit**. Negative is allowed — "no cheese, less 20" is a real option.
    pub price_delta: Money,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn line() -> SaleLine {
        SaleLine {
            id: Uuid::from_u128(1),
            product_id: Uuid::from_u128(2),
            name: "Basmati rice 5kg".to_owned(),
            unit_price: Money::from_minor(48_000, Currency::Bdt),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            discount: Discount::None,
            modifiers: Vec::new(),
            void: None,
        }
    }

    #[test]
    fn a_fresh_line_is_active() {
        assert!(line().is_active());
    }

    #[test]
    fn a_voided_line_is_retained_but_inactive() {
        // The evidence has to survive: void patterns are the strongest fraud signal a POS has.
        let mut voided = line();
        voided.void = Some(LineVoid {
            reason: VoidReason::Mistake,
            authorized_by: Uuid::from_u128(9),
        });

        assert!(!voided.is_active());
        assert_eq!(
            voided.name, "Basmati rice 5kg",
            "the line itself is not erased"
        );
    }

    #[test]
    fn tax_input_carries_the_snapshot_not_a_lookup() {
        let input = line().to_tax_input().expect("prices");
        assert_eq!(input.unit_price, Money::from_minor(48_000, Currency::Bdt));
        assert_eq!(input.tax_class, TaxClass::standard(1500));
    }

    #[test]
    fn options_add_to_the_unit_price_before_quantity_applies() {
        // The only reading that makes two lattes with an extra shot each cost two shots. Adding
        // afterwards would charge for one.
        let mut latte = line();
        latte.unit_price = Money::from_minor(32_000, Currency::Bdt);
        latte.quantity = Quantity::from_milli(2_000);
        latte.modifiers = vec![Modifier {
            option_id: Uuid::from_u128(9),
            name: "Extra shot".to_owned(),
            price_delta: Money::from_minor(5_000, Currency::Bdt),
        }];

        assert_eq!(
            latte.effective_unit_price(),
            Ok(Money::from_minor(37_000, Currency::Bdt))
        );

        let input = latte.to_tax_input().expect("prices");
        assert_eq!(input.unit_price, Money::from_minor(37_000, Currency::Bdt));
        assert_eq!(input.quantity, Quantity::from_milli(2_000));
    }

    #[test]
    fn a_negative_option_reduces_the_unit_price() {
        // "No cheese, less 20" is a real option, and the delta is genuinely negative.
        let mut burger = line();
        burger.unit_price = Money::from_minor(25_000, Currency::Bdt);
        burger.modifiers = vec![Modifier {
            option_id: Uuid::from_u128(9),
            name: "No cheese".to_owned(),
            price_delta: Money::from_minor(-2_000, Currency::Bdt),
        }];

        assert_eq!(
            burger.effective_unit_price(),
            Ok(Money::from_minor(23_000, Currency::Bdt))
        );
        assert_eq!(
            burger.modifier_total(),
            Ok(Money::from_minor(-2_000, Currency::Bdt))
        );
    }

    #[test]
    fn several_options_accumulate() {
        let mut drink = line();
        drink.unit_price = Money::from_minor(32_000, Currency::Bdt);
        drink.modifiers = vec![
            Modifier {
                option_id: Uuid::from_u128(9),
                name: "Extra shot".to_owned(),
                price_delta: Money::from_minor(5_000, Currency::Bdt),
            },
            Modifier {
                option_id: Uuid::from_u128(10),
                name: "Oat milk".to_owned(),
                price_delta: Money::from_minor(3_000, Currency::Bdt),
            },
            Modifier {
                option_id: Uuid::from_u128(11),
                name: "No sugar".to_owned(),
                price_delta: Money::from_minor(0, Currency::Bdt),
            },
        ];

        assert_eq!(
            drink.effective_unit_price(),
            Ok(Money::from_minor(40_000, Currency::Bdt))
        );
    }

    #[test]
    fn a_line_with_no_options_prices_exactly_as_before() {
        // Retail is the degenerate café here too: the same code path, with an empty vector.
        let plain = line();
        assert!(plain.modifiers.is_empty());
        assert_eq!(plain.effective_unit_price(), Ok(plain.unit_price));
    }
}
