use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::Money;
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
    #[must_use]
    pub const fn to_tax_input(&self) -> LineInput {
        LineInput {
            unit_price: self.unit_price,
            quantity: self.quantity,
            tax_class: self.tax_class,
            discount: self.discount,
        }
    }
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
        let input = line().to_tax_input();
        assert_eq!(input.unit_price, Money::from_minor(48_000, Currency::Bdt));
        assert_eq!(input.tax_class, TaxClass::standard(1500));
    }
}
