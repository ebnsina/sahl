//! The catalogue, rebuilt from events.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::event::CatalogueEvent;
use super::product::{CatalogueError, Product};

/// Everything a shop sells.
///
/// `BTreeMap` because this reaches the sell screen, reports and sync payloads, where hash order
/// would differ between processes and a product grid would reshuffle itself between launches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalogue {
    products: BTreeMap<Uuid, Product>,
}

impl Catalogue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild from a stream of events.
    ///
    /// # Errors
    /// [`CatalogueError`] if the stream is inconsistent.
    pub fn replay(events: &[CatalogueEvent]) -> Result<Self, CatalogueError> {
        let mut catalogue = Self::new();
        for event in events {
            catalogue.apply(event)?;
        }
        Ok(catalogue)
    }

    /// Apply one event.
    ///
    /// # Errors
    /// [`CatalogueError`] if the event is not valid for the current state.
    pub fn apply(&mut self, event: &CatalogueEvent) -> Result<(), CatalogueError> {
        match event {
            CatalogueEvent::ProductAdded { product_id, .. } => {
                if self.products.contains_key(product_id) {
                    return Err(CatalogueError::Duplicate {
                        product_id: *product_id,
                    });
                }
                let product = event.to_product(true)?;
                self.assert_barcodes_free(&product)?;
                self.products.insert(*product_id, product);
            }

            CatalogueEvent::ProductUpdated { product_id, .. } => {
                // An update keeps whether the product is currently sellable. Editing a withdrawn
                // product's price should not quietly put it back on the sell screen.
                let active = self
                    .products
                    .get(product_id)
                    .ok_or(CatalogueError::Unknown {
                        product_id: *product_id,
                    })?
                    .active;

                let product = event.to_product(active)?;
                self.assert_barcodes_free(&product)?;
                self.products.insert(*product_id, product);
            }

            CatalogueEvent::ProductWithdrawn { product_id, .. } => {
                self.product_mut(*product_id)?.active = false;
            }

            CatalogueEvent::ProductRestored { product_id, .. } => {
                self.product_mut(*product_id)?.active = true;
            }
        }

        Ok(())
    }

    /// Refuse a barcode already on a different product.
    ///
    /// A scan has to resolve to one thing. Two products sharing a code means the till either picks
    /// arbitrarily or asks — and asking at a counter, on every scan, is not a product anyone uses.
    fn assert_barcodes_free(&self, product: &Product) -> Result<(), CatalogueError> {
        for barcode in &product.barcodes {
            if let Some(existing) = self
                .products
                .values()
                .find(|other| other.id != product.id && other.barcodes.contains(barcode))
            {
                return Err(CatalogueError::DuplicateBarcode {
                    barcode: barcode.clone(),
                    product_id: existing.id,
                });
            }
        }
        Ok(())
    }

    /// What a scanner found, if anything.
    ///
    /// Withdrawn products are still matched. A code on a shelf outlives its removal from the
    /// screen, and "that product is withdrawn" is a far better answer at a counter than silence.
    #[must_use]
    pub fn by_barcode(&self, barcode: &str) -> Option<&Product> {
        self.products
            .values()
            .find(|product| product.barcodes.iter().any(|code| code == barcode))
    }

    #[must_use]
    pub fn get(&self, product_id: Uuid) -> Option<&Product> {
        self.products.get(&product_id)
    }

    /// Everything currently on the sell screen, by name.
    ///
    /// Sorted by name rather than id: a cashier scans a grid visually, and id order is arbitrary
    /// to everyone except the database.
    #[must_use]
    pub fn sellable(&self) -> Vec<&Product> {
        let mut found: Vec<&Product> = self
            .products
            .values()
            .filter(|product| product.active)
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        found
    }

    /// Everything, including withdrawn products.
    #[must_use]
    pub fn all(&self) -> Vec<&Product> {
        let mut found: Vec<&Product> = self.products.values().collect();
        found.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        found
    }

    /// The categories in use, in display order.
    #[must_use]
    pub fn categories(&self) -> Vec<String> {
        let mut found: Vec<String> = self
            .products
            .values()
            .filter(|product| product.active)
            .filter_map(|product| product.category.clone())
            .collect();
        found.sort();
        found.dedup();
        found
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.products.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.products.len()
    }

    fn product_mut(&mut self, product_id: Uuid) -> Result<&mut Product, CatalogueError> {
        self.products
            .get_mut(&product_id)
            .ok_or(CatalogueError::Unknown { product_id })
    }
}

#[cfg(test)]
mod tests {
    use super::super::event::ProductDetails;
    use super::super::product::Unit;
    use super::*;
    use crate::money::{Currency, Money};
    use crate::tax::TaxClass;
    use crate::time::Timestamp;

    const BDT: Currency = Currency::Bdt;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(n: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + n)
    }

    fn details(name: &str, minor: i64, barcodes: &[&str]) -> ProductDetails {
        ProductDetails {
            name: name.to_owned(),
            sku: None,
            barcodes: barcodes.iter().map(|code| (*code).to_owned()).collect(),
            price: Money::from_minor(minor, BDT),
            unit: Unit::Piece,
            tax_class: TaxClass::standard(1500),
            category: Some("Staples".to_owned()),
            option_groups: Vec::new(),
        }
    }

    fn added(product: u128, name: &str, minor: i64, barcodes: &[&str]) -> CatalogueEvent {
        CatalogueEvent::ProductAdded {
            product_id: id(product),
            details: details(name, minor, barcodes),
            at: at(0),
            added_by: id(0x0E),
        }
    }

    fn shop() -> Catalogue {
        Catalogue::replay(&[
            added(1, "Basmati rice 5kg", 48_000, &["8901"]),
            added(2, "Cooking oil 2L", 34_000, &["8902"]),
        ])
        .expect("valid")
    }

    #[test]
    fn products_are_added_and_sellable() {
        let catalogue = shop();
        assert_eq!(catalogue.len(), 2);
        assert_eq!(catalogue.sellable().len(), 2);
    }

    #[test]
    fn the_sell_screen_is_ordered_by_name_not_by_id() {
        // A cashier scans a grid visually; id order is arbitrary to everyone but the database.
        let catalogue = Catalogue::replay(&[
            added(9, "Aata 2kg", 12_000, &["8909"]),
            added(1, "Zeera 100g", 9_000, &["8901"]),
        ])
        .expect("valid");

        let names: Vec<&str> = catalogue
            .sellable()
            .iter()
            .map(|product| product.name.as_str())
            .collect();
        assert_eq!(names, vec!["Aata 2kg", "Zeera 100g"]);
    }

    #[test]
    fn a_scan_finds_the_product() {
        assert_eq!(shop().by_barcode("8902").expect("found").id, id(2));
        assert!(shop().by_barcode("nope").is_none());
    }

    #[test]
    fn a_barcode_cannot_be_on_two_products() {
        // A scan has to resolve to one thing; asking a cashier which one, on every scan, is not a
        // product anyone uses.
        let result = Catalogue::replay(&[
            added(1, "Rice", 48_000, &["8901"]),
            added(2, "Rice, other importer", 49_000, &["8901"]),
        ]);

        assert_eq!(
            result,
            Err(CatalogueError::DuplicateBarcode {
                barcode: "8901".to_owned(),
                product_id: id(1)
            })
        );
    }

    #[test]
    fn a_product_may_carry_several_barcodes() {
        // The same good from two importers carries two EANs, and both are on the shelf.
        let catalogue =
            Catalogue::replay(&[added(1, "Rice", 48_000, &["8901", "8911"])]).expect("valid");

        assert_eq!(catalogue.by_barcode("8901").expect("found").id, id(1));
        assert_eq!(catalogue.by_barcode("8911").expect("found").id, id(1));
    }

    #[test]
    fn keeping_its_own_barcode_on_an_update_is_not_a_clash() {
        let mut catalogue = shop();
        catalogue
            .apply(&CatalogueEvent::ProductUpdated {
                product_id: id(1),
                details: details("Basmati rice 5kg", 52_000, &["8901"]),
                at: at(10),
                updated_by: id(0x0E),
            })
            .expect("updates");

        assert_eq!(
            catalogue.get(id(1)).expect("present").price,
            Money::from_minor(52_000, BDT)
        );
    }

    #[test]
    fn a_withdrawn_product_leaves_the_screen_but_not_the_catalogue() {
        // Past sales reference it. A sale pointing at nothing is a report nobody can read and a
        // recall nobody can trace.
        let mut catalogue = shop();
        catalogue
            .apply(&CatalogueEvent::ProductWithdrawn {
                product_id: id(1),
                at: at(10),
                withdrawn_by: id(0x0E),
            })
            .expect("withdraws");

        assert_eq!(catalogue.sellable().len(), 1);
        assert_eq!(catalogue.len(), 2);
        assert_eq!(
            catalogue.get(id(1)).expect("present").name,
            "Basmati rice 5kg"
        );
    }

    #[test]
    fn a_withdrawn_product_still_answers_a_scan() {
        // A code on a shelf outlives its removal from the screen, and "that is withdrawn" beats
        // silence at a counter.
        let mut catalogue = shop();
        catalogue
            .apply(&CatalogueEvent::ProductWithdrawn {
                product_id: id(1),
                at: at(10),
                withdrawn_by: id(0x0E),
            })
            .expect("withdraws");

        let found = catalogue.by_barcode("8901").expect("still found");
        assert!(!found.active);
    }

    #[test]
    fn editing_a_withdrawn_product_does_not_put_it_back_on_sale() {
        // Correcting a price is not the same decision as restocking it.
        let mut catalogue = shop();
        catalogue
            .apply(&CatalogueEvent::ProductWithdrawn {
                product_id: id(1),
                at: at(10),
                withdrawn_by: id(0x0E),
            })
            .expect("withdraws");
        catalogue
            .apply(&CatalogueEvent::ProductUpdated {
                product_id: id(1),
                details: details("Basmati rice 5kg", 52_000, &["8901"]),
                at: at(11),
                updated_by: id(0x0E),
            })
            .expect("updates");

        assert!(!catalogue.get(id(1)).expect("present").active);
    }

    #[test]
    fn a_restored_product_returns() {
        let mut catalogue = shop();
        for event in [
            CatalogueEvent::ProductWithdrawn {
                product_id: id(1),
                at: at(10),
                withdrawn_by: id(0x0E),
            },
            CatalogueEvent::ProductRestored {
                product_id: id(1),
                at: at(11),
                restored_by: id(0x0E),
            },
        ] {
            catalogue.apply(&event).expect("applies");
        }

        assert_eq!(catalogue.sellable().len(), 2);
    }

    #[test]
    fn adding_the_same_product_twice_is_refused() {
        let result = Catalogue::replay(&[
            added(1, "Rice", 48_000, &["8901"]),
            added(1, "Rice again", 48_000, &["8999"]),
        ]);
        assert_eq!(result, Err(CatalogueError::Duplicate { product_id: id(1) }));
    }

    #[test]
    fn editing_a_product_that_does_not_exist_is_refused() {
        let mut catalogue = shop();
        let result = catalogue.apply(&CatalogueEvent::ProductUpdated {
            product_id: id(99),
            details: details("Ghost", 1_000, &[]),
            at: at(10),
            updated_by: id(0x0E),
        });
        assert_eq!(result, Err(CatalogueError::Unknown { product_id: id(99) }));
    }

    #[test]
    fn categories_come_back_deduplicated_and_sorted() {
        let catalogue = Catalogue::replay(&[
            CatalogueEvent::ProductAdded {
                product_id: id(1),
                details: ProductDetails {
                    category: Some("Drinks".to_owned()),
                    ..details("Tea", 32_000, &["8901"])
                },
                at: at(0),
                added_by: id(0x0E),
            },
            added(2, "Rice", 48_000, &["8902"]),
            added(3, "Oil", 34_000, &["8903"]),
        ])
        .expect("valid");

        assert_eq!(catalogue.categories(), vec!["Drinks", "Staples"]);
    }

    #[test]
    fn replay_is_deterministic() {
        // This drives the sell screen's layout; a grid that reshuffles between launches is a grid
        // a cashier cannot learn.
        let events = vec![
            added(2, "Cooking oil 2L", 34_000, &["8902"]),
            added(1, "Basmati rice 5kg", 48_000, &["8901"]),
            CatalogueEvent::ProductWithdrawn {
                product_id: id(2),
                at: at(10),
                withdrawn_by: id(0x0E),
            },
        ];

        assert_eq!(
            Catalogue::replay(&events).expect("valid"),
            Catalogue::replay(&events).expect("valid")
        );
    }
}
