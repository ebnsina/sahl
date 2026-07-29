//! Batch-level stock, and picking which batch a sale draws from.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::MoneyError;
use crate::quantity::Quantity;
use crate::time::Timestamp;

use super::batch::Batch;

/// One batch and what remains of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchLevel {
    pub batch: Batch,
    pub on_hand: Quantity,
}

/// A slice of a pick: take this much from this batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allocation {
    pub batch_id: Uuid,
    pub taken: Quantity,
}

/// The result of trying to fill a quantity from stock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pick {
    pub allocations: Vec<Allocation>,
    /// What could not be covered. Non-zero means the shelf disagrees with the book.
    ///
    /// Returned rather than refused, for the same reason oversell is flagged and not blocked: the
    /// shopkeeper can see the goods, and a till that argues with them stops being used.
    pub shortfall: Quantity,
}

impl Pick {
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.shortfall.is_zero()
    }

    /// Total actually allocated.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] on overflow.
    pub fn taken(&self) -> Result<Quantity, MoneyError> {
        self.allocations
            .iter()
            .try_fold(Quantity::ZERO, |total, allocation| {
                total.checked_add(allocation.taken)
            })
    }
}

/// Fill `wanted` from `levels`, soonest-expiring first.
///
/// Expired batches are skipped entirely rather than sold down. That is the difference between a
/// grocery and a pharmacy being able to use this at all: expired stock is not cheap stock, it is
/// stock that must not leave the shelf, and a system that quietly sells it is worse than one with
/// no batch tracking.
///
/// Splitting across batches is normal — three of an item may genuinely come from two crates — and
/// the split is recorded so a recall can trace every unit.
///
/// # Errors
/// [`MoneyError::Overflow`] on overflow.
pub fn pick_fefo(
    levels: &[BatchLevel],
    wanted: Quantity,
    now: Timestamp,
) -> Result<Pick, MoneyError> {
    if !wanted.milli().is_positive() {
        return Ok(Pick {
            allocations: Vec::new(),
            shortfall: Quantity::ZERO,
        });
    }

    let mut usable: Vec<&BatchLevel> = levels
        .iter()
        .filter(|level| !level.batch.is_expired(now) && level.on_hand.milli() > 0)
        .collect();
    usable.sort_by_key(|level| level.batch.fefo_key());

    let mut remaining = wanted.milli();
    let mut allocations = Vec::new();

    for level in usable {
        if remaining <= 0 {
            break;
        }
        let take = remaining.min(level.on_hand.milli());
        allocations.push(Allocation {
            batch_id: level.batch.id,
            taken: Quantity::from_milli(take),
        });
        remaining = remaining.saturating_sub(take);
    }

    Ok(Pick {
        allocations,
        shortfall: Quantity::from_milli(remaining.max(0)),
    })
}

/// Total across every batch of a product, expired included.
///
/// Expired stock is still physically present and still has to be counted, written off, and
/// explained — excluding it here would make the book disagree with the shelf.
///
/// # Errors
/// [`MoneyError::Overflow`] on overflow.
pub fn total_on_hand(levels: &[BatchLevel]) -> Result<Quantity, MoneyError> {
    levels.iter().try_fold(Quantity::ZERO, |total, level| {
        total.checked_add(level.on_hand)
    })
}

/// Stock that is sellable right now.
///
/// # Errors
/// [`MoneyError::Overflow`] on overflow.
pub fn sellable_on_hand(levels: &[BatchLevel], now: Timestamp) -> Result<Quantity, MoneyError> {
    levels
        .iter()
        .filter(|level| !level.batch.is_expired(now))
        .try_fold(Quantity::ZERO, |total, level| {
            total.checked_add(level.on_hand)
        })
}

/// Batches already past their date but still on the shelf.
///
/// The write-off list. Sorted by expiry so the oldest problem is first.
#[must_use]
pub fn expired(levels: &[BatchLevel], now: Timestamp) -> Vec<&BatchLevel> {
    let mut found: Vec<&BatchLevel> = levels
        .iter()
        .filter(|level| level.batch.is_expired(now) && level.on_hand.milli() > 0)
        .collect();
    found.sort_by_key(|level| level.batch.fefo_key());
    found
}

/// Batches expiring within `window_millis`.
///
/// The discount-or-use list, which is where the money actually is: stock sold at a markdown beats
/// stock written off.
#[must_use]
pub fn expiring_soon(
    levels: &[BatchLevel],
    now: Timestamp,
    window_millis: i64,
) -> Vec<&BatchLevel> {
    let mut found: Vec<&BatchLevel> = levels
        .iter()
        .filter(|level| level.batch.expires_within(now, window_millis) && level.on_hand.milli() > 0)
        .collect();
    found.sort_by_key(|level| level.batch.fefo_key());
    found
}

/// Which batches a product's stock sits in, keyed for stable reporting.
///
/// `BTreeMap` because this reaches reports and sync payloads, where hash order would differ between
/// processes.
#[must_use]
pub fn by_product(levels: &[BatchLevel]) -> BTreeMap<Uuid, Vec<&BatchLevel>> {
    let mut grouped: BTreeMap<Uuid, Vec<&BatchLevel>> = BTreeMap::new();
    for level in levels {
        grouped
            .entry(level.batch.product_id)
            .or_default()
            .push(level);
    }
    for batches in grouped.values_mut() {
        batches.sort_by_key(|level| level.batch.fefo_key());
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn day(n: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + n * 86_400_000)
    }

    const PRODUCT: u128 = 0x99;
    const WEEK: i64 = 86_400_000 * 7;

    fn level(n: u128, expiry: Option<i64>, received: i64, on_hand_milli: i64) -> BatchLevel {
        BatchLevel {
            batch: Batch {
                id: id(n),
                product_id: id(PRODUCT),
                lot: Some(format!("LOT{n}")),
                expires_at: expiry.map(day),
                received_at: day(received),
            },
            on_hand: Quantity::from_milli(on_hand_milli),
        }
    }

    #[test]
    fn a_pick_takes_the_soonest_expiring_batch_first() {
        let levels = vec![level(1, Some(30), 0, 5_000), level(2, Some(5), 1, 5_000)];
        let pick = pick_fefo(&levels, Quantity::from_milli(3_000), day(0)).expect("picks");

        assert!(pick.is_complete());
        assert_eq!(
            pick.allocations,
            vec![Allocation {
                batch_id: id(2),
                taken: Quantity::from_milli(3_000)
            }]
        );
    }

    #[test]
    fn a_pick_splits_across_batches_when_one_is_not_enough() {
        // Three of an item genuinely coming from two crates — recorded so a recall can trace them.
        let levels = vec![level(1, Some(30), 0, 5_000), level(2, Some(5), 1, 2_000)];
        let pick = pick_fefo(&levels, Quantity::from_milli(4_000), day(0)).expect("picks");

        assert!(pick.is_complete());
        assert_eq!(pick.allocations.len(), 2);
        assert_eq!(pick.allocations[0].batch_id, id(2), "soonest first");
        assert_eq!(pick.allocations[0].taken, Quantity::from_milli(2_000));
        assert_eq!(pick.allocations[1].taken, Quantity::from_milli(2_000));
        assert_eq!(pick.taken(), Ok(Quantity::from_milli(4_000)));
    }

    #[test]
    fn expired_stock_is_never_picked() {
        // The line between a grocery being able to use this and not. Expired stock is not cheap
        // stock; it is stock that must not leave the shelf.
        let levels = vec![level(1, Some(-1), -10, 9_000), level(2, Some(30), 0, 1_000)];
        let pick = pick_fefo(&levels, Quantity::from_milli(5_000), day(0)).expect("picks");

        assert_eq!(pick.allocations.len(), 1);
        assert_eq!(pick.allocations[0].batch_id, id(2));
        assert_eq!(
            pick.shortfall,
            Quantity::from_milli(4_000),
            "the expired crate does not fill the gap"
        );
    }

    #[test]
    fn an_unfillable_pick_reports_the_shortfall_rather_than_refusing() {
        // Same reasoning as oversell: the shopkeeper can see the goods.
        let levels = vec![level(1, Some(30), 0, 1_000)];
        let pick = pick_fefo(&levels, Quantity::from_milli(4_000), day(0)).expect("picks");

        assert!(!pick.is_complete());
        assert_eq!(pick.shortfall, Quantity::from_milli(3_000));
        assert_eq!(pick.taken(), Ok(Quantity::from_milli(1_000)));
    }

    #[test]
    fn picking_from_nothing_is_all_shortfall() {
        let pick = pick_fefo(&[], Quantity::from_milli(2_000), day(0)).expect("picks");
        assert_eq!(pick.shortfall, Quantity::from_milli(2_000));
        assert!(pick.allocations.is_empty());
    }

    #[test]
    fn picking_zero_takes_nothing() {
        let levels = vec![level(1, Some(30), 0, 5_000)];
        let pick = pick_fefo(&levels, Quantity::ZERO, day(0)).expect("picks");

        assert!(pick.allocations.is_empty());
        assert!(pick.is_complete());
    }

    #[test]
    fn a_pick_is_deterministic_regardless_of_input_order() {
        // Two devices computing the same pick must agree, or a recall traces different units.
        let forwards = vec![
            level(1, Some(10), 0, 1_000),
            level(2, Some(10), 1, 1_000),
            level(3, Some(5), 2, 1_000),
        ];
        let backwards: Vec<_> = forwards.iter().rev().cloned().collect();

        assert_eq!(
            pick_fefo(&forwards, Quantity::from_milli(2_500), day(0)),
            pick_fefo(&backwards, Quantity::from_milli(2_500), day(0))
        );
    }

    #[test]
    fn expired_stock_still_counts_toward_what_is_physically_present() {
        // It has to be counted, written off and explained; excluding it makes the book disagree
        // with the shelf.
        let levels = vec![level(1, Some(-1), -10, 9_000), level(2, Some(30), 0, 1_000)];

        assert_eq!(total_on_hand(&levels), Ok(Quantity::from_milli(10_000)));
        assert_eq!(
            sellable_on_hand(&levels, day(0)),
            Ok(Quantity::from_milli(1_000))
        );
    }

    #[test]
    fn the_write_off_list_puts_the_oldest_problem_first() {
        let levels = vec![
            level(1, Some(-1), -5, 1_000),
            level(2, Some(-9), -20, 2_000),
            level(3, Some(30), 0, 3_000),
        ];
        let found = expired(&levels, day(0));

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].batch.id, id(2), "expired longest ago first");
    }

    #[test]
    fn the_discount_list_covers_only_the_window() {
        let levels = vec![
            level(1, Some(3), 0, 1_000),
            level(2, Some(20), 0, 1_000),
            level(3, Some(-1), -5, 1_000),
            level(4, None, 0, 1_000),
        ];
        let soon = expiring_soon(&levels, day(0), WEEK);

        assert_eq!(soon.len(), 1);
        assert_eq!(soon[0].batch.id, id(1));
    }

    #[test]
    fn an_emptied_batch_drops_off_both_lists() {
        let levels = vec![level(1, Some(-1), -5, 0), level(2, Some(3), 0, 0)];

        assert!(expired(&levels, day(0)).is_empty());
        assert!(expiring_soon(&levels, day(0), WEEK).is_empty());
    }

    #[test]
    fn grouping_by_product_is_stable_and_fefo_ordered() {
        let other = 0xAA;
        let mut levels = vec![level(1, Some(30), 0, 1_000), level(2, Some(5), 1, 1_000)];
        levels.push(BatchLevel {
            batch: Batch {
                id: id(3),
                product_id: id(other),
                lot: None,
                expires_at: None,
                received_at: day(0),
            },
            on_hand: Quantity::from_milli(1_000),
        });

        let grouped = by_product(&levels);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[&id(PRODUCT)][0].batch.id, id(2), "soonest first");
        assert_eq!(by_product(&levels), grouped, "stable across calls");
    }
}
