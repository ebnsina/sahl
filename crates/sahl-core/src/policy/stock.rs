//! Stock, and what to do when two tills sell the same last unit.
//!
//! Stock is **derived** — the sum of its movements — so two devices can never produce a merge
//! conflict. What they can produce is an oversell: both sold the last bag of rice while apart, and
//! both were right at the time.
//!
//! The rule is to detect and flag, not to block. A till that refuses a sale because a *sibling*
//! might have sold the item is a till that stops working the moment the network does, which is the
//! failure this product exists to avoid. The shopkeeper knows what is on the shelf; the software's
//! job is to tell them the count is now wrong, not to argue.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::quantity::Quantity;

/// Why stock moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MovementReason {
    Sale,
    Refund,
    Received,
    /// A physical count corrected the book.
    Counted,
    Wastage,
    TransferIn,
    TransferOut,
}

/// One change to a product's stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StockMovement {
    pub product_id: Uuid,
    /// Negative for anything leaving the shelf.
    pub delta: Quantity,
    pub reason: MovementReason,
    /// Which till recorded it — the input to per-device oversell attribution.
    pub device_id: Uuid,
}

/// Whether a sale would take stock below zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockVerdict {
    /// Enough on hand.
    Available,
    /// Not enough — by this much.
    ///
    /// Still sellable when the outlet allows it. The name is deliberate: this is a *shortfall to
    /// report*, not a refusal.
    Short { by: Quantity },
}

impl StockVerdict {
    #[must_use]
    pub const fn is_short(self) -> bool {
        matches!(self, Self::Short { .. })
    }
}

/// Sum movements into a current level.
///
/// # Errors
/// [`crate::MoneyError::Overflow`] on overflow.
pub fn level_of(
    movements: &[StockMovement],
    product_id: Uuid,
) -> Result<Quantity, crate::MoneyError> {
    movements
        .iter()
        .filter(|movement| movement.product_id == product_id)
        .try_fold(Quantity::ZERO, |total, movement| {
            total.checked_add(movement.delta)
        })
}

/// Check a proposed sale against stock on hand.
#[must_use]
pub fn check(on_hand: Quantity, wanted: Quantity) -> StockVerdict {
    let remaining = on_hand.milli().saturating_sub(wanted.milli());
    if remaining >= 0 {
        StockVerdict::Available
    } else {
        StockVerdict::Short {
            by: Quantity::from_milli(remaining.saturating_abs()),
        }
    }
}

/// An oversell found after two tills' events were merged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Oversell {
    pub product_id: Uuid,
    /// How far below zero the book went.
    pub shortfall: Quantity,
    /// Devices that sold it. Two or more means the tills were apart when it happened, which is
    /// worth saying differently to an owner than one till selling stock it knew it lacked.
    pub devices: Vec<Uuid>,
}

/// Find products whose merged movements went negative.
///
/// Run after sync. A negative level is not an accusation — it usually means two tills sold the same
/// last unit while apart, or that the shelf count was wrong to begin with. Either way it is
/// something the owner should see rather than something the software should hide.
///
/// # Errors
/// [`crate::MoneyError::Overflow`] on overflow.
pub fn detect_oversells(movements: &[StockMovement]) -> Result<Vec<Oversell>, crate::MoneyError> {
    // BTreeMap, not HashMap: this output reaches a report and a sync payload, and hash iteration
    // order differs between processes.
    let mut products: std::collections::BTreeMap<Uuid, (i64, Vec<Uuid>)> =
        std::collections::BTreeMap::new();

    for movement in movements {
        let entry = products
            .entry(movement.product_id)
            .or_insert((0, Vec::new()));
        entry.0 = entry
            .0
            .checked_add(movement.delta.milli())
            .ok_or(crate::MoneyError::Overflow)?;

        if movement.delta.is_negative() && !entry.1.contains(&movement.device_id) {
            entry.1.push(movement.device_id);
        }
    }

    Ok(products
        .into_iter()
        .filter_map(|(product_id, (level, mut devices))| {
            if level >= 0 {
                return None;
            }
            devices.sort_unstable();
            Some(Oversell {
                product_id,
                shortfall: Quantity::from_milli(level.saturating_abs()),
                devices,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    const RICE: u128 = 0x21;
    const OIL: u128 = 0x22;
    const TILL_A: u128 = 0xA;
    const TILL_B: u128 = 0xB;

    fn moved(product: u128, milli: i64, reason: MovementReason, device: u128) -> StockMovement {
        StockMovement {
            product_id: id(product),
            delta: Quantity::from_milli(milli),
            reason,
            device_id: id(device),
        }
    }

    #[test]
    fn a_level_is_the_sum_of_its_movements() {
        let movements = vec![
            moved(RICE, 10_000, MovementReason::Received, TILL_A),
            moved(RICE, -3_000, MovementReason::Sale, TILL_A),
            moved(OIL, -1_000, MovementReason::Sale, TILL_A),
        ];
        assert_eq!(
            level_of(&movements, id(RICE)),
            Ok(Quantity::from_milli(7_000))
        );
    }

    #[test]
    fn enough_stock_permits_the_sale() {
        assert_eq!(
            check(Quantity::from_milli(5_000), Quantity::from_milli(2_000)),
            StockVerdict::Available
        );
    }

    #[test]
    fn a_shortfall_is_reported_with_its_size_not_a_refusal() {
        // The software's job is to say the count is wrong, not to argue with the shopkeeper.
        let verdict = check(Quantity::from_milli(1_000), Quantity::from_milli(2_500));
        assert_eq!(
            verdict,
            StockVerdict::Short {
                by: Quantity::from_milli(1_500)
            }
        );
        assert!(verdict.is_short());
    }

    #[test]
    fn selling_exactly_the_last_unit_is_available() {
        assert_eq!(check(Quantity::ONE, Quantity::ONE), StockVerdict::Available);
    }

    #[test]
    fn a_balanced_book_reports_no_oversell() {
        let movements = vec![
            moved(RICE, 10_000, MovementReason::Received, TILL_A),
            moved(RICE, -4_000, MovementReason::Sale, TILL_A),
            moved(RICE, -6_000, MovementReason::Sale, TILL_B),
        ];
        assert_eq!(detect_oversells(&movements), Ok(Vec::new()));
    }

    #[test]
    fn two_tills_selling_the_last_unit_apart_is_caught_and_attributed() {
        // The headline case. Both were right at the time; the book is now wrong and the owner
        // should hear about it from the software rather than from a customer.
        let movements = vec![
            moved(RICE, 1_000, MovementReason::Received, TILL_A),
            moved(RICE, -1_000, MovementReason::Sale, TILL_A),
            moved(RICE, -1_000, MovementReason::Sale, TILL_B),
        ];

        let found = detect_oversells(&movements).expect("no overflow");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].shortfall, Quantity::from_milli(1_000));
        assert_eq!(
            found[0].devices,
            vec![id(TILL_A), id(TILL_B)],
            "both tills named, so the owner can tell this from one till overselling alone"
        );
    }

    #[test]
    fn a_single_till_overselling_names_only_itself() {
        // Different situation, different conversation: this one knew and sold anyway.
        let movements = vec![moved(RICE, -2_000, MovementReason::Sale, TILL_A)];
        let found = detect_oversells(&movements).expect("no overflow");

        assert_eq!(found[0].devices, vec![id(TILL_A)]);
    }

    #[test]
    fn a_receipt_after_the_fact_clears_the_oversell() {
        // Stock going negative then positive again is a delivery arriving late, not a problem.
        let movements = vec![
            moved(RICE, -2_000, MovementReason::Sale, TILL_A),
            moved(RICE, 5_000, MovementReason::Received, TILL_A),
        ];
        assert_eq!(detect_oversells(&movements), Ok(Vec::new()));
    }

    #[test]
    fn oversells_are_reported_in_a_stable_order() {
        // This output reaches a report and a sync payload; hash order would differ per process.
        let movements = vec![
            moved(OIL, -1_000, MovementReason::Sale, TILL_A),
            moved(RICE, -1_000, MovementReason::Sale, TILL_B),
        ];

        let first = detect_oversells(&movements).expect("no overflow");
        let second = detect_oversells(&movements).expect("no overflow");
        assert_eq!(first, second);
        assert!(
            first[0].product_id < first[1].product_id,
            "sorted by product"
        );
    }

    #[test]
    fn a_refund_does_not_count_as_a_selling_device() {
        // Only outward movements attribute an oversell; a refund is stock coming back.
        let movements = vec![
            moved(RICE, -3_000, MovementReason::Sale, TILL_A),
            moved(RICE, 1_000, MovementReason::Refund, TILL_B),
        ];
        let found = detect_oversells(&movements).expect("no overflow");

        assert_eq!(found[0].devices, vec![id(TILL_A)]);
    }
}
