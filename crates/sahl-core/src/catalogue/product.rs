//! What a shop sells.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::money::Money;
use crate::tax::TaxClass;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CatalogueError {
    #[error("no product {product_id}")]
    Unknown { product_id: Uuid },

    #[error("product {product_id} already exists")]
    Duplicate { product_id: Uuid },

    #[error("{field} cannot be blank")]
    Blank { field: &'static str },

    #[error("a price cannot be negative, got {price}")]
    NegativePrice { price: Money },

    #[error("barcode {barcode} is already on product {product_id}")]
    DuplicateBarcode { barcode: String, product_id: Uuid },
}

/// How a supply is counted.
///
/// This is the Mushak 6.3 "Unit of Supply" column and the reason the catalogue had to exist before
/// a challan could be correct — every line printed "pcs" until a product could say otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Unit {
    /// Sold as whole items. Quantity is always a whole number.
    Piece,
    Kilogram,
    Gram,
    Litre,
    Millilitre,
    Metre,
    /// A container sold as one thing — a sack of rice, a crate of bottles.
    Pack,
}

impl Unit {
    /// The abbreviation printed on a receipt and a challan.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Piece => "pcs",
            Self::Kilogram => "kg",
            Self::Gram => "g",
            Self::Litre => "L",
            Self::Millilitre => "ml",
            Self::Metre => "m",
            Self::Pack => "pack",
        }
    }

    /// Whether this unit can be sold in fractions.
    ///
    /// Drives whether a scale is offered and whether a fractional quantity is a typo. Selling 0.4
    /// of a piece is a mis-key; selling 0.4 kg is Tuesday.
    #[must_use]
    pub const fn is_divisible(self) -> bool {
        !matches!(self, Self::Piece | Self::Pack)
    }

    /// Parse a stored label.
    ///
    /// # Errors
    /// [`CatalogueError::Blank`] for anything unrecognised — a unit that silently became pieces
    /// would put the wrong figure in a Mushak column.
    pub fn from_label(label: &str) -> Result<Self, CatalogueError> {
        match label {
            "pcs" => Ok(Self::Piece),
            "kg" => Ok(Self::Kilogram),
            "g" => Ok(Self::Gram),
            "L" => Ok(Self::Litre),
            "ml" => Ok(Self::Millilitre),
            "m" => Ok(Self::Metre),
            "pack" => Ok(Self::Pack),
            _ => Err(CatalogueError::Blank { field: "unit" }),
        }
    }

    /// Every unit, for a picker.
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::Piece,
            Self::Kilogram,
            Self::Gram,
            Self::Litre,
            Self::Millilitre,
            Self::Metre,
            Self::Pack,
        ]
    }
}

/// Something a shop sells.
///
/// The price here is the *current* price. A sale line snapshots what it charged at the time, so
/// changing this never rewrites history — which is exactly what makes last-writer-wins safe for
/// catalogue edits arriving out of order from two devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Product {
    pub id: Uuid,
    pub name: String,
    /// The shop's own code. Not required — plenty of small shops have none.
    pub sku: Option<String>,
    /// Scanned codes. Several per product is normal: a multipack and a single often differ, and
    /// the same good from two importers carries two EANs.
    pub barcodes: Vec<String>,
    /// Price per unit, tax-inclusive or not according to the outlet's pricing mode.
    pub price: Money,
    pub unit: Unit,
    pub tax_class: TaxClass,
    /// Grouping for reports and for the sell screen's layout.
    pub category: Option<String>,
    /// Whether it appears on the sell screen. Withdrawn products stay in the catalogue because
    /// past sales reference them, and a sale pointing at nothing is a report nobody can read.
    pub active: bool,
}

impl Product {
    /// Check a product is one a shop could actually sell.
    ///
    /// # Errors
    /// [`CatalogueError`] naming the field that is wrong.
    pub fn validate(&self) -> Result<(), CatalogueError> {
        if self.name.trim().is_empty() {
            return Err(CatalogueError::Blank { field: "name" });
        }
        if self.price.minor().is_negative() {
            // A negative price is a refund dressed as a product, and it would let anyone move money
            // out of a till by ringing a sale.
            return Err(CatalogueError::NegativePrice { price: self.price });
        }
        if self.barcodes.iter().any(|code| code.trim().is_empty()) {
            return Err(CatalogueError::Blank { field: "barcode" });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn product() -> Product {
        Product {
            id: Uuid::from_u128(1),
            name: "Basmati rice 5kg".to_owned(),
            sku: Some("RICE-5".to_owned()),
            barcodes: vec!["8901234567890".to_owned()],
            price: Money::from_minor(48_000, Currency::Bdt),
            unit: Unit::Piece,
            tax_class: TaxClass::standard(1500),
            category: Some("Staples".to_owned()),
            active: true,
        }
    }

    #[test]
    fn a_complete_product_is_accepted() {
        assert_eq!(product().validate(), Ok(()));
    }

    #[test]
    fn a_blank_name_is_refused() {
        let nameless = Product {
            name: "   ".to_owned(),
            ..product()
        };
        assert_eq!(
            nameless.validate(),
            Err(CatalogueError::Blank { field: "name" })
        );
    }

    #[test]
    fn a_negative_price_is_refused() {
        // A refund dressed as a product would let anyone move money out of a till by ringing a sale.
        let bad = Product {
            price: Money::from_minor(-100, Currency::Bdt),
            ..product()
        };
        assert!(matches!(
            bad.validate(),
            Err(CatalogueError::NegativePrice { .. })
        ));
    }

    #[test]
    fn a_free_product_is_allowed() {
        // Zero is not negative. Samples and promotional items are real.
        let free = Product {
            price: Money::from_minor(0, Currency::Bdt),
            ..product()
        };
        assert_eq!(free.validate(), Ok(()));
    }

    #[test]
    fn units_round_trip_through_their_printed_label() {
        // The label reaches a Mushak column and a receipt, so it is a wire format.
        for unit in Unit::all() {
            assert_eq!(Unit::from_label(unit.label()), Ok(unit), "{unit:?}");
        }
    }

    #[test]
    fn an_unknown_unit_is_refused_rather_than_defaulted() {
        // Silently becoming pieces would put the wrong figure in a Mushak column.
        assert!(Unit::from_label("dozen").is_err());
    }

    #[test]
    fn only_measured_units_divide() {
        // Selling 0.4 of a piece is a mis-key; selling 0.4 kg is Tuesday.
        assert!(!Unit::Piece.is_divisible());
        assert!(!Unit::Pack.is_divisible());
        assert!(Unit::Kilogram.is_divisible());
        assert!(Unit::Litre.is_divisible());
    }
}
