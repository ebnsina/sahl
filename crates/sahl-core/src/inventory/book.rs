//! Batch levels, rebuilt from inventory events.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::money::{Money, MoneyError};
use crate::quantity::Quantity;
use crate::time::Timestamp;

use super::batch::Batch;
use super::event::InventoryEvent;
use super::ledger::BatchLevel;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InventoryError {
    #[error("quantity error: {0}")]
    Money(#[from] MoneyError),

    #[error("no batch {batch_id}; it was never received")]
    UnknownBatch { batch_id: Uuid },

    #[error("batch {batch_id} was already received")]
    DuplicateBatch { batch_id: Uuid },

    #[error("a movement of {quantity} is not a positive amount")]
    NonPositiveMovement { quantity: Quantity },

    #[error("a count cannot be negative, got {counted}")]
    NegativeCount { counted: Quantity },
}

/// A count that disagreed with the book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountVariance {
    pub batch_id: Uuid,
    pub expected: Quantity,
    pub counted: Quantity,
    /// Counted minus expected. Negative means stock is missing.
    pub delta: Quantity,
    pub at: Timestamp,
    pub counted_by: Uuid,
}

/// Every batch the outlet knows about.
///
/// `BTreeMap` throughout: this feeds reports and sync payloads, where hash order would differ
/// between processes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryBook {
    levels: BTreeMap<Uuid, BatchLevel>,
    costs: BTreeMap<Uuid, Money>,
    variances: Vec<CountVariance>,
}

impl InventoryBook {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild from a stream of events.
    ///
    /// # Errors
    /// [`InventoryError`] if the stream is inconsistent.
    pub fn replay(events: &[InventoryEvent]) -> Result<Self, InventoryError> {
        let mut book = Self::new();
        for event in events {
            book.apply(event)?;
        }
        Ok(book)
    }

    /// Apply one event.
    ///
    /// # Errors
    /// [`InventoryError`] if the event is not valid for the current state.
    pub fn apply(&mut self, event: &InventoryEvent) -> Result<(), InventoryError> {
        match event {
            InventoryEvent::BatchReceived {
                batch_id,
                product_id,
                lot,
                expires_at,
                quantity,
                unit_cost,
                at,
                ..
            } => {
                if self.levels.contains_key(batch_id) {
                    return Err(InventoryError::DuplicateBatch {
                        batch_id: *batch_id,
                    });
                }
                if !quantity.milli().is_positive() {
                    return Err(InventoryError::NonPositiveMovement {
                        quantity: *quantity,
                    });
                }

                self.levels.insert(
                    *batch_id,
                    BatchLevel {
                        batch: Batch {
                            id: *batch_id,
                            product_id: *product_id,
                            lot: lot.clone(),
                            expires_at: *expires_at,
                            received_at: *at,
                        },
                        on_hand: *quantity,
                    },
                );
                self.costs.insert(*batch_id, *unit_cost);
            }

            InventoryEvent::StockIssued {
                batch_id, quantity, ..
            } => {
                Self::assert_positive(*quantity)?;
                let level = self.level_mut(*batch_id)?;
                // Allowed to go negative. The shelf is the authority, and a book that refuses to
                // record what a shopkeeper physically did is a book they stop maintaining — the
                // negative is the signal, and a count will correct it.
                level.on_hand = level.on_hand.checked_add(quantity.checked_neg()?)?;
            }

            InventoryEvent::StockReturned {
                batch_id, quantity, ..
            } => {
                Self::assert_positive(*quantity)?;
                let level = self.level_mut(*batch_id)?;
                level.on_hand = level.on_hand.checked_add(*quantity)?;
            }

            InventoryEvent::BatchCounted {
                batch_id,
                counted,
                at,
                counted_by,
            } => {
                if counted.is_negative() {
                    return Err(InventoryError::NegativeCount { counted: *counted });
                }
                let level = self.level_mut(*batch_id)?;
                let expected = level.on_hand;
                let delta = counted.checked_add(expected.checked_neg()?)?;

                // The count wins — it is the physical truth — but the disagreement is kept. A
                // count that silently overwrote the book would erase the only evidence that stock
                // went missing.
                level.on_hand = *counted;

                if !delta.is_zero() {
                    self.variances.push(CountVariance {
                        batch_id: *batch_id,
                        expected,
                        counted: *counted,
                        delta,
                        at: *at,
                        counted_by: *counted_by,
                    });
                }
            }
        }

        Ok(())
    }

    fn assert_positive(quantity: Quantity) -> Result<(), InventoryError> {
        if quantity.milli().is_positive() {
            Ok(())
        } else {
            Err(InventoryError::NonPositiveMovement { quantity })
        }
    }

    fn level_mut(&mut self, batch_id: Uuid) -> Result<&mut BatchLevel, InventoryError> {
        self.levels
            .get_mut(&batch_id)
            .ok_or(InventoryError::UnknownBatch { batch_id })
    }

    #[must_use]
    pub fn levels(&self) -> Vec<BatchLevel> {
        self.levels.values().cloned().collect()
    }

    #[must_use]
    pub fn level(&self, batch_id: Uuid) -> Option<&BatchLevel> {
        self.levels.get(&batch_id)
    }

    /// What a batch cost per unit when it was received.
    #[must_use]
    pub fn unit_cost(&self, batch_id: Uuid) -> Option<Money> {
        self.costs.get(&batch_id).copied()
    }

    /// Every count that disagreed with the book, oldest first.
    ///
    /// The shrinkage record. One batch off by a little is noise; the same batch off every count is
    /// the thing an owner wants to know about.
    #[must_use]
    pub fn variances(&self) -> &[CountVariance] {
        &self.variances
    }

    /// Batches whose recorded level has gone below zero.
    ///
    /// Means stock left that the book never saw arrive — a delivery not entered, or an issue
    /// recorded twice.
    #[must_use]
    pub fn negative_batches(&self) -> Vec<&BatchLevel> {
        self.levels
            .values()
            .filter(|level| level.on_hand.is_negative())
            .collect()
    }

    /// Batches of one product, in FEFO order — what a pick draws from.
    #[must_use]
    pub fn for_product(&self, product_id: Uuid) -> Vec<BatchLevel> {
        let mut found: Vec<BatchLevel> = self
            .levels
            .values()
            .filter(|level| level.batch.product_id == product_id)
            .cloned()
            .collect();
        found.sort_by_key(|level| level.batch.fefo_key());
        found
    }
}
