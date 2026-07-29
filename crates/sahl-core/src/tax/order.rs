use serde::{Deserialize, Serialize};

use crate::money::{Currency, Money, Rounding};
use crate::quantity::Quantity;

use super::class::TaxClass;
use super::discount::Discount;

/// Whether quoted prices already contain tax.
///
/// This is not a preference — it changes what the customer is charged, and it differs by market.
/// Bangladeshi retail overwhelmingly quotes a tax-inclusive MRP printed on the packet, and Gulf B2C
/// pricing is inclusive too. Getting this wrong doesn't produce a rounding discrepancy; it produces
/// a bill 15% off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingMode {
    /// Quoted prices exclude tax; tax is added on top.
    TaxExclusive,
    /// Quoted prices already include tax; tax is extracted from within.
    ///
    /// The default, because it matches how both target markets price at retail — and because a
    /// ৳100 shelf label must ring up as exactly ৳100.
    #[default]
    TaxInclusive,
}

/// One sellable line on an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineInput {
    /// Price per whole unit, in the order's currency.
    pub unit_price: Money,
    /// How many units — fractional for weighed goods.
    pub quantity: Quantity,
    /// VAT treatment of this supply.
    pub tax_class: TaxClass,
    /// Reduction applied to this line before any order-level discount.
    pub discount: Discount,
}

impl LineInput {
    /// A whole-unit line at a standard rate with no discount — the common retail case.
    #[must_use]
    pub const fn new(unit_price: Money, quantity: Quantity, tax_class: TaxClass) -> Self {
        Self {
            unit_price,
            quantity,
            tax_class,
            discount: Discount::None,
        }
    }

    /// Attach a line-level discount.
    #[must_use]
    pub const fn with_discount(mut self, discount: Discount) -> Self {
        self.discount = discount;
        self
    }
}

/// A complete order awaiting calculation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderInput {
    /// Currency for the whole order. Every line must match.
    pub currency: Currency,
    /// Whether line prices include tax.
    pub pricing_mode: PricingMode,
    /// How fractions of a minor unit are resolved. Half-up for both target jurisdictions.
    pub rounding: Rounding,
    /// The lines.
    pub lines: Vec<LineInput>,
    /// A reduction applied to the order as a whole, apportioned back across lines.
    pub order_discount: Discount,
}

impl OrderInput {
    /// A tax-inclusive, half-up order in `currency` — the default posture for both target markets.
    #[must_use]
    pub const fn new(currency: Currency, lines: Vec<LineInput>) -> Self {
        Self {
            currency,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
            lines,
            order_discount: Discount::None,
        }
    }

    /// Switch to tax-exclusive pricing.
    #[must_use]
    pub const fn tax_exclusive(mut self) -> Self {
        self.pricing_mode = PricingMode::TaxExclusive;
        self
    }

    /// Attach an order-level discount.
    #[must_use]
    pub const fn with_order_discount(mut self, discount: Discount) -> Self {
        self.order_discount = discount;
        self
    }
}
