//! Splitting a bill.
//!
//! A split is **arithmetic, not a new kind of transaction.** Three people paying separately is three
//! tenders against one sale, and the sale has recorded partial tenders and computed a balance due
//! since P1. Nothing here writes an event; it works out what each part should be and the ordinary
//! tender path takes it from there.
//!
//! That framing is the whole reason this module is small. Modelling a split as its own aggregate
//! would mean a second place that decides when a sale is paid, and two answers to "is this settled"
//! is how a café hands back a table that still owes money.
//!
//! What *does* need care is that the parts sum to the total exactly. A bill of 100 split three ways
//! is not three amounts of 33.33 — that loses a cent every time, and a cent lost per split across a
//! service is a till that never reconciles. Largest-remainder allocation is what makes it exact, and
//! it has been in `Money` since P0 for precisely this.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::{Money, MoneyError};

use super::line::SaleLine;

/// One person's share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitPart {
    /// One-based, as a cashier counts them aloud.
    pub number: u32,
    pub amount: Money,
    /// The lines this part covers, when the split was by item. Empty for an even split, where
    /// nobody is paying for anything in particular.
    pub line_ids: Vec<Uuid>,
}

/// Divide a total evenly.
///
/// The remainder goes to the earliest parts, one minor unit each. Somebody has to absorb it — a
/// bill of 100 across three cannot be three equal amounts — and doing it deterministically means
/// two devices computing the same split agree, which matters the moment a second till is involved.
///
/// # Errors
/// [`MoneyError::InvalidWeights`] if `ways` is zero; [`MoneyError`] on overflow.
pub fn evenly(total: Money, ways: u32) -> Result<Vec<SplitPart>, MoneyError> {
    if ways == 0 {
        return Err(MoneyError::InvalidWeights);
    }

    let weights = vec![1_u64; ways as usize];
    let amounts = total.allocate(&weights)?;

    Ok(amounts
        .into_iter()
        .enumerate()
        .map(|(index, amount)| SplitPart {
            number: u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1),
            amount,
            line_ids: Vec::new(),
        })
        .collect())
}

/// Divide by who ordered what.
///
/// `assignment` gives, for each part, the line ids that part is paying for. A line left out of every
/// part is charged to nobody, which would silently under-collect — so that is refused rather than
/// absorbed.
///
/// Line totals come from the calculated order rather than being recomputed here, so an apportioned
/// order-level discount lands exactly where the tax engine put it. Recomputing would be a second
/// implementation of the apportionment, and the two disagreeing is a bill that does not add up.
///
/// # Errors
/// [`SplitError`] if a line is unassigned, assigned twice, or unknown.
pub fn by_lines(
    lines: &[SaleLine],
    line_totals: &[Money],
    assignment: &[Vec<Uuid>],
) -> Result<Vec<SplitPart>, SplitError> {
    if assignment.is_empty() {
        return Err(SplitError::NoParts);
    }
    if lines.len() != line_totals.len() {
        return Err(SplitError::Mismatched {
            lines: lines.len(),
            totals: line_totals.len(),
        });
    }

    let active: Vec<(Uuid, Money)> = lines
        .iter()
        .zip(line_totals.iter())
        .filter(|(line, _)| line.is_active())
        .map(|(line, total)| (line.id, *total))
        .collect();

    // Every active line must be paid for exactly once. Assigned twice double-charges the table;
    // left out under-collects and nobody notices until the drawer is counted.
    let mut seen: Vec<Uuid> = Vec::new();
    for part in assignment {
        for line_id in part {
            if !active.iter().any(|(id, _)| id == line_id) {
                return Err(SplitError::UnknownLine { line_id: *line_id });
            }
            if seen.contains(line_id) {
                return Err(SplitError::LineAssignedTwice { line_id: *line_id });
            }
            seen.push(*line_id);
        }
    }
    if let Some((missing, _)) = active.iter().find(|(id, _)| !seen.contains(id)) {
        return Err(SplitError::LineUnassigned { line_id: *missing });
    }

    let currency = active.first().map_or_else(
        || Money::from_minor(0, crate::Currency::Bdt).currency(),
        |(_, money)| money.currency(),
    );

    let mut parts = Vec::with_capacity(assignment.len());
    for (index, line_ids) in assignment.iter().enumerate() {
        let mut amount = Money::from_minor(0, currency);
        for line_id in line_ids {
            let line_total = active
                .iter()
                .find(|(id, _)| id == line_id)
                .map(|(_, money)| *money)
                .ok_or(SplitError::UnknownLine { line_id: *line_id })?;
            amount = amount.checked_add(line_total)?;
        }
        parts.push(SplitPart {
            number: u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1),
            amount,
            line_ids: line_ids.clone(),
        });
    }

    Ok(parts)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SplitError {
    #[error("arithmetic error: {0}")]
    Money(#[from] MoneyError),

    #[error("a split needs at least one part")]
    NoParts,

    #[error("{lines} lines but {totals} calculated totals")]
    Mismatched { lines: usize, totals: usize },

    #[error("line {line_id} is not on this sale")]
    UnknownLine { line_id: Uuid },

    #[error("line {line_id} was assigned to two parts")]
    LineAssignedTwice { line_id: Uuid },

    #[error("line {line_id} was not assigned to anyone")]
    LineUnassigned { line_id: Uuid },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;
    use crate::quantity::Quantity;
    use crate::tax::{Discount, TaxClass};

    const BDT: Currency = Currency::Bdt;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, BDT)
    }

    fn line(n: u128) -> SaleLine {
        SaleLine {
            id: id(n),
            product_id: id(n + 100),
            name: format!("Item {n}"),
            unit_price: bdt(10_000),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            discount: Discount::None,
            modifiers: Vec::new(),
            void: None,
        }
    }

    fn sum(parts: &[SplitPart]) -> i64 {
        parts.iter().map(|part| part.amount.minor()).sum()
    }

    #[test]
    fn an_even_split_that_divides_cleanly_is_equal() {
        let parts = evenly(bdt(30_000), 3).expect("splits");
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|part| part.amount == bdt(10_000)));
    }

    #[test]
    fn an_even_split_that_does_not_divide_loses_nothing() {
        // A bill of 100 across three is the case that costs a cent every time it is done wrong, and
        // a cent per split across a service is a till that never reconciles.
        let parts = evenly(bdt(10_000), 3).expect("splits");

        assert_eq!(sum(&parts), 10_000, "the parts are the whole bill");
        assert_eq!(parts[0].amount, bdt(3_334), "the remainder lands first");
        assert_eq!(parts[1].amount, bdt(3_333));
        assert_eq!(parts[2].amount, bdt(3_333));
    }

    #[test]
    fn an_even_split_is_deterministic() {
        // Two tills computing the same split must agree, which matters the moment a second device
        // is involved in one table.
        assert_eq!(evenly(bdt(10_001), 7), evenly(bdt(10_001), 7));
    }

    #[test]
    fn parts_are_numbered_from_one_as_a_cashier_counts_them() {
        let parts = evenly(bdt(9_000), 3).expect("splits");
        let numbers: Vec<u32> = parts.iter().map(|part| part.number).collect();
        assert_eq!(numbers, vec![1, 2, 3]);
    }

    #[test]
    fn splitting_zero_ways_is_refused() {
        assert_eq!(evenly(bdt(10_000), 0), Err(MoneyError::InvalidWeights));
    }

    #[test]
    fn splitting_one_way_is_the_whole_bill() {
        let parts = evenly(bdt(10_000), 1).expect("splits");
        assert_eq!(parts[0].amount, bdt(10_000));
    }

    #[test]
    fn an_item_split_charges_each_part_for_what_it_took() {
        let lines = vec![line(1), line(2), line(3)];
        let totals = vec![bdt(11_500), bdt(23_000), bdt(5_000)];
        let parts = by_lines(&lines, &totals, &[vec![id(1), id(3)], vec![id(2)]]).expect("splits");

        assert_eq!(parts[0].amount, bdt(16_500));
        assert_eq!(parts[1].amount, bdt(23_000));
        assert_eq!(sum(&parts), 39_500, "and together, the whole bill");
    }

    #[test]
    fn a_line_nobody_claimed_is_refused() {
        // Charged to nobody would silently under-collect, and nobody notices until the drawer is
        // counted at the end of the night.
        let lines = vec![line(1), line(2)];
        let totals = vec![bdt(11_500), bdt(23_000)];

        assert_eq!(
            by_lines(&lines, &totals, &[vec![id(1)]]),
            Err(SplitError::LineUnassigned { line_id: id(2) })
        );
    }

    #[test]
    fn a_line_claimed_twice_is_refused() {
        // Double-charging the table is the mirror failure, and the customer finds that one.
        let lines = vec![line(1), line(2)];
        let totals = vec![bdt(11_500), bdt(23_000)];

        assert_eq!(
            by_lines(&lines, &totals, &[vec![id(1)], vec![id(1), id(2)]]),
            Err(SplitError::LineAssignedTwice { line_id: id(1) })
        );
    }

    #[test]
    fn a_voided_line_belongs_to_nobody() {
        // It contributes nothing to the total, so requiring it to be assigned would make every
        // split of a corrected bill impossible.
        let mut voided = line(2);
        voided.void = Some(super::super::line::LineVoid {
            reason: super::super::line::VoidReason::Mistake,
            authorized_by: id(9),
        });

        let lines = vec![line(1), voided];
        let totals = vec![bdt(11_500), bdt(0)];
        let parts = by_lines(&lines, &totals, &[vec![id(1)]]).expect("splits");

        assert_eq!(parts[0].amount, bdt(11_500));
    }

    #[test]
    fn a_line_from_another_sale_is_refused() {
        let lines = vec![line(1)];
        let totals = vec![bdt(11_500)];

        assert_eq!(
            by_lines(&lines, &totals, &[vec![id(99)]]),
            Err(SplitError::UnknownLine { line_id: id(99) })
        );
    }

    #[test]
    fn a_split_with_no_parts_is_refused() {
        assert_eq!(
            by_lines(&[line(1)], &[bdt(1)], &[]),
            Err(SplitError::NoParts)
        );
    }

    #[test]
    fn an_empty_part_is_allowed_and_owes_nothing() {
        // Somebody at the table who ordered nothing still counts as a person, and refusing the
        // split because of it would be pedantry a waiter has to work around.
        let lines = vec![line(1)];
        let totals = vec![bdt(11_500)];
        let parts = by_lines(&lines, &totals, &[vec![id(1)], Vec::new()]).expect("splits");

        assert_eq!(parts[1].amount, bdt(0));
        assert_eq!(sum(&parts), 11_500);
    }
}
