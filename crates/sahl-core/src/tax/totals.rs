use serde::{Deserialize, Serialize};

use crate::money::Money;

use super::class::TaxClass;

/// What one line came to, after every discount and with tax resolved.
///
/// `net + tax == total` holds exactly, always.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineTotals {
    /// `unit_price × quantity`, before any discount.
    pub gross: Money,
    /// This line's own discount plus its apportioned share of the order discount.
    pub discount: Money,
    /// Taxable base, excluding tax.
    pub net: Money,
    /// Tax charged on this line.
    pub tax: Money,
    /// What the customer pays for this line: `net + tax`.
    pub total: Money,
    /// VAT treatment applied.
    pub tax_class: TaxClass,
}

/// Tax accumulated for one tax class — the VAT summary block on an invoice.
///
/// Fiscal documents in both target markets require this breakdown: Mushak 6.3 shows it, and ZATCA's
/// UBL invoice carries one `TaxSubtotal` per category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxGroup {
    /// The class these amounts belong to.
    pub tax_class: TaxClass,
    /// Sum of the net amounts taxed at this class.
    pub taxable_base: Money,
    /// Tax charged at this class.
    pub tax: Money,
}

/// The calculated order.
///
/// Every aggregate here is the exact sum of its parts — see the property tests. Nothing is
/// recomputed from a rounded subtotal, because that is precisely how an invoice ends up with a
/// summary that disagrees with its own lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderTotals {
    /// Per-line results, in input order.
    pub lines: Vec<LineTotals>,
    /// VAT summary, sorted into conventional invoice order.
    pub tax_groups: Vec<TaxGroup>,
    /// Sum of line gross amounts, before discounts.
    pub gross: Money,
    /// Total discount given, line-level and order-level combined.
    pub discount: Money,
    /// Total excluding tax.
    pub net: Money,
    /// Total tax.
    pub tax: Money,
    /// What the customer pays.
    pub total: Money,
}
