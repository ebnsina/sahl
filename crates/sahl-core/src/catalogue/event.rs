//! Catalogue events.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventPayload;
use crate::money::Money;
use crate::tax::TaxClass;
use crate::time::Timestamp;

use super::product::{CatalogueError, Product, Unit};

/// The editable facts about a product.
///
/// A full replacement rather than a patch. Two devices editing the same product while apart cannot
/// have their patches merged into a state either of them intended, and the resolution rule the plan
/// settled on — last writer wins by server sequence — only makes sense over whole values. It is
/// safe precisely because every sale line snapshots its own price, so history never moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductDetails {
    pub name: String,
    pub sku: Option<String>,
    pub barcodes: Vec<String>,
    pub price: Money,
    pub unit: Unit,
    pub tax_class: TaxClass,
    pub category: Option<String>,
    /// Choices offered when this is rung. `default` so events written before options existed still
    /// deserialize, and their recorded hashes stay valid.
    #[serde(default)]
    pub option_groups: Vec<super::options::ModifierGroup>,
}

/// Everything that happens to the catalogue.
///
/// Kind strings are hashed into the chain, so they are a wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CatalogueEvent {
    /// A product was added.
    ProductAdded {
        product_id: Uuid,
        details: ProductDetails,
        at: Timestamp,
        added_by: Uuid,
    },

    /// A product's details changed — a price rise, a corrected name, a new barcode.
    ProductUpdated {
        product_id: Uuid,
        details: ProductDetails,
        at: Timestamp,
        updated_by: Uuid,
    },

    /// Taken off the sell screen.
    ///
    /// Withdrawn rather than deleted: past sales reference it, and a sale pointing at nothing is a
    /// report nobody can read and a recall nobody can trace.
    ProductWithdrawn {
        product_id: Uuid,
        at: Timestamp,
        withdrawn_by: Uuid,
    },

    /// Put back on the sell screen.
    ProductRestored {
        product_id: Uuid,
        at: Timestamp,
        restored_by: Uuid,
    },
}

impl CatalogueEvent {
    #[must_use]
    pub const fn product_id(&self) -> Uuid {
        match self {
            Self::ProductAdded { product_id, .. }
            | Self::ProductUpdated { product_id, .. }
            | Self::ProductWithdrawn { product_id, .. }
            | Self::ProductRestored { product_id, .. } => *product_id,
        }
    }

    #[must_use]
    pub const fn at(&self) -> Timestamp {
        match self {
            Self::ProductAdded { at, .. }
            | Self::ProductUpdated { at, .. }
            | Self::ProductWithdrawn { at, .. }
            | Self::ProductRestored { at, .. } => *at,
        }
    }

    /// The product these details describe.
    ///
    /// # Errors
    /// [`CatalogueError`] if the details would not be sellable.
    pub fn to_product(&self, active: bool) -> Result<Product, CatalogueError> {
        match self {
            Self::ProductAdded {
                product_id,
                details,
                ..
            }
            | Self::ProductUpdated {
                product_id,
                details,
                ..
            } => {
                let product = Product {
                    id: *product_id,
                    name: details.name.clone(),
                    sku: details.sku.clone(),
                    barcodes: details.barcodes.clone(),
                    price: details.price,
                    unit: details.unit,
                    tax_class: details.tax_class,
                    category: details.category.clone(),
                    option_groups: details.option_groups.clone(),
                    active,
                };
                product.validate()?;
                Ok(product)
            }
            Self::ProductWithdrawn { product_id, .. }
            | Self::ProductRestored { product_id, .. } => Err(CatalogueError::Unknown {
                product_id: *product_id,
            }),
        }
    }
}

impl EventPayload for CatalogueEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::ProductAdded { .. } => "catalogue.product_added",
            Self::ProductUpdated { .. } => "catalogue.product_updated",
            Self::ProductWithdrawn { .. } => "catalogue.product_withdrawn",
            Self::ProductRestored { .. } => "catalogue.product_restored",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn details() -> ProductDetails {
        ProductDetails {
            name: "Basmati rice 5kg".to_owned(),
            sku: Some("RICE-5".to_owned()),
            barcodes: vec!["8901234567890".to_owned()],
            price: Money::from_minor(48_000, Currency::Bdt),
            unit: Unit::Piece,
            tax_class: TaxClass::standard(1500),
            category: Some("Staples".to_owned()),
            option_groups: Vec::new(),
        }
    }

    #[test]
    fn kind_strings_are_stable_and_namespaced() {
        // Hashed into the chain; a rename invalidates every catalogue edit already recorded.
        let added = CatalogueEvent::ProductAdded {
            product_id: Uuid::from_u128(1),
            details: details(),
            at: Timestamp::from_millis(0),
            added_by: Uuid::from_u128(2),
        };

        assert_eq!(added.kind(), "catalogue.product_added");
        assert_eq!(added.product_id(), Uuid::from_u128(1));

        let encoded = serde_json::to_string(&added).expect("serialises");
        assert!(encoded.contains(r#""type":"product_added""#));
        assert!(encoded.contains(r#""unit":"piece""#));
        assert_eq!(
            serde_json::from_str::<CatalogueEvent>(&encoded).expect("deserialises"),
            added
        );
    }

    #[test]
    fn an_unsellable_product_is_refused_on_replay() {
        // A till must not adopt a product it could not legally ring, whatever emitted it.
        let bad = CatalogueEvent::ProductAdded {
            product_id: Uuid::from_u128(1),
            details: ProductDetails {
                price: Money::from_minor(-1, Currency::Bdt),
                ..details()
            },
            at: Timestamp::from_millis(0),
            added_by: Uuid::from_u128(2),
        };

        assert!(matches!(
            bad.to_product(true),
            Err(CatalogueError::NegativePrice { .. })
        ));
    }
}
