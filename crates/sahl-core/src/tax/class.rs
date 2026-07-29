use serde::{Deserialize, Serialize};

use crate::money::Rate;

/// How a supply is treated for VAT.
///
/// The arithmetic for [`TaxClass::ZeroRated`] and [`TaxClass::Exempt`] is identical — both add zero
/// tax — but they are **legally distinct** and must be reported separately. A zero-rated supply is
/// taxable at 0% and the merchant may reclaim input VAT against it; an exempt supply is outside the
/// tax entirely and they may not.
///
/// Collapsing them into "rate = 0" would make the arithmetic correct and the filing wrong, which is
/// why this is an enum rather than a bare [`Rate`]. Both NBR's Mushak returns and ZATCA's invoice
/// XML require the distinction on the face of the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaxClass {
    /// Taxable at the given rate. Bangladesh's ladder runs 15/7.5/5/4.5/2.4%; KSA's standard is 15%.
    Standard { rate: Rate },
    /// Taxable at 0%. Input VAT remains reclaimable — exports, and certain listed goods.
    ZeroRated,
    /// Outside VAT. Input VAT is not reclaimable.
    Exempt,
}

impl TaxClass {
    /// Convenience constructor for a standard rate in basis points.
    #[must_use]
    pub const fn standard(basis_points: i32) -> Self {
        Self::Standard {
            rate: Rate::from_basis_points(basis_points),
        }
    }

    /// The rate to apply. Zero for both zero-rated and exempt supplies.
    #[must_use]
    pub const fn rate(self) -> Rate {
        match self {
            Self::Standard { rate } => rate,
            Self::ZeroRated | Self::Exempt => Rate::ZERO,
        }
    }

    /// Whether this supply carries tax at all.
    #[must_use]
    pub const fn is_taxable(self) -> bool {
        match self {
            Self::Standard { rate } => !rate.is_zero(),
            Self::ZeroRated | Self::Exempt => false,
        }
    }

    /// Stable ordering key for grouping on a printed invoice and in fiscal returns.
    ///
    /// Standard rates sort ascending first, then zero-rated, then exempt — the conventional layout
    /// of a VAT summary block, and stable so terminal and server produce byte-identical documents.
    #[must_use]
    pub const fn sort_key(self) -> (u8, i32) {
        match self {
            Self::Standard { rate } => (0, rate.basis_points()),
            Self::ZeroRated => (1, 0),
            Self::Exempt => (2, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_rated_and_exempt_are_arithmetically_equal_but_not_the_same_thing() {
        assert_eq!(TaxClass::ZeroRated.rate(), TaxClass::Exempt.rate());
        assert_ne!(TaxClass::ZeroRated, TaxClass::Exempt);
    }

    #[test]
    fn a_zero_percent_standard_rate_is_not_taxable() {
        assert!(!TaxClass::standard(0).is_taxable());
        assert!(TaxClass::standard(1500).is_taxable());
    }

    #[test]
    fn summary_groups_sort_into_conventional_invoice_order() {
        let mut classes = vec![
            TaxClass::Exempt,
            TaxClass::standard(1500),
            TaxClass::ZeroRated,
            TaxClass::standard(750),
        ];
        classes.sort_by_key(|class| class.sort_key());

        assert_eq!(
            classes,
            vec![
                TaxClass::standard(750),
                TaxClass::standard(1500),
                TaxClass::ZeroRated,
                TaxClass::Exempt,
            ]
        );
    }
}
