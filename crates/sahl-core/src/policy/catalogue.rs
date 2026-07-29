//! Resolving concurrent catalogue edits.
//!
//! Last-writer-wins by server sequence, which is only safe because **every sale line snapshots its
//! price at the moment of sale**. A price change landing at 3pm cannot alter a receipt printed at
//! 2pm, so losing an edit costs a merchant a re-entry, never a wrong total.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A catalogue edit, tagged with where the server placed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueEdit<T> {
    pub product_id: Uuid,
    /// Assigned by the server on ingest — the only ordering both sides agree on.
    ///
    /// Not a device clock: two tills' clocks disagree, and a merchant editing on a device with a
    /// fast clock would silently win every conflict.
    pub server_seq: i64,
    pub device_id: Uuid,
    pub value: T,
}

/// Pick the surviving edit.
///
/// Ties break on device id so the result is total and identical everywhere. A tie means the server
/// assigned one sequence to two edits, which it does not do — the branch exists so the function
/// cannot return nothing.
#[must_use]
pub fn resolve<T: Clone>(a: &CatalogueEdit<T>, b: &CatalogueEdit<T>) -> CatalogueEdit<T> {
    match a.server_seq.cmp(&b.server_seq) {
        core::cmp::Ordering::Greater => a.clone(),
        core::cmp::Ordering::Less => b.clone(),
        core::cmp::Ordering::Equal => {
            if a.device_id >= b.device_id {
                a.clone()
            } else {
                b.clone()
            }
        }
    }
}

/// Fold a run of edits down to the surviving value per product.
///
/// # Panics
/// Never — `resolve` is total.
#[must_use]
pub fn latest_per_product<T: Clone>(
    edits: &[CatalogueEdit<T>],
) -> std::collections::BTreeMap<Uuid, CatalogueEdit<T>> {
    let mut winners: std::collections::BTreeMap<Uuid, CatalogueEdit<T>> =
        std::collections::BTreeMap::new();

    for edit in edits {
        winners
            .entry(edit.product_id)
            .and_modify(|held| *held = resolve(held, edit))
            .or_insert_with(|| edit.clone());
    }
    winners
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn edit(product: u128, seq: i64, device: u128, price: i64) -> CatalogueEdit<i64> {
        CatalogueEdit {
            product_id: id(product),
            server_seq: seq,
            device_id: id(device),
            value: price,
        }
    }

    #[test]
    fn the_later_edit_wins() {
        let earlier = edit(1, 10, 0xA, 4_800);
        let later = edit(1, 20, 0xB, 5_200);

        assert_eq!(resolve(&earlier, &later).value, 5_200);
        assert_eq!(resolve(&later, &earlier).value, 5_200);
    }

    #[test]
    fn resolution_does_not_depend_on_argument_order() {
        let a = edit(1, 7, 0xA, 100);
        let b = edit(1, 9, 0xB, 200);
        assert_eq!(resolve(&a, &b), resolve(&b, &a));
    }

    #[test]
    fn a_tie_is_broken_deterministically_rather_than_left_open() {
        let a = edit(1, 5, 0xA, 100);
        let b = edit(1, 5, 0xB, 200);
        assert_eq!(resolve(&a, &b), resolve(&b, &a));
    }

    #[test]
    fn each_product_keeps_its_own_winner() {
        let edits = vec![
            edit(1, 10, 0xA, 4_800),
            edit(2, 11, 0xA, 9_000),
            edit(1, 12, 0xB, 5_200),
        ];
        let winners = latest_per_product(&edits);

        assert_eq!(winners.len(), 2);
        assert_eq!(winners[&id(1)].value, 5_200);
        assert_eq!(winners[&id(2)].value, 9_000);
    }

    #[test]
    fn the_fold_is_independent_of_arrival_order() {
        // Pull pages can deliver edits in any order after an outage.
        let forwards = vec![
            edit(1, 1, 0xA, 100),
            edit(1, 2, 0xB, 200),
            edit(1, 3, 0xA, 300),
        ];
        let backwards: Vec<_> = forwards.iter().rev().cloned().collect();

        assert_eq!(
            latest_per_product(&forwards),
            latest_per_product(&backwards)
        );
    }
}
