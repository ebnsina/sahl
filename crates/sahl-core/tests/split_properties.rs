//! Splitting a bill must not lose or invent a cent.
//!
//! The example tests cover the cases someone thought of. These cover the ones nobody did: any total,
//! any number of ways, any assignment of lines. A cent lost per split across a service is a till
//! that never reconciles, and the person who finds it is a shopkeeper at midnight with a drawer that
//! is short and no idea why.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use proptest::prelude::*;
use sahl_core::money::{Currency, Money};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{Modifier, SaleLine};
use sahl_core::sale::{by_lines, evenly};
use sahl_core::tax::{Discount, TaxClass};
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;

fn bdt(minor: i64) -> Money {
    Money::from_minor(minor, BDT)
}

fn line(n: u128) -> SaleLine {
    SaleLine {
        id: Uuid::from_u128(n),
        product_id: Uuid::from_u128(n + 1_000),
        name: format!("Item {n}"),
        unit_price: bdt(10_000),
        quantity: Quantity::ONE,
        tax_class: TaxClass::standard(1500),
        discount: Discount::None,
        modifiers: Vec::<Modifier>::new(),
        void: None,
    }
}

proptest! {
    #[test]
    fn an_even_split_always_sums_to_the_bill(
        total in 0_i64..100_000_000,
        ways in 1_u32..40,
    ) {
        let parts = evenly(bdt(total), ways).expect("splits");
        let summed: i64 = parts.iter().map(|part| part.amount.minor()).sum();

        prop_assert_eq!(summed, total, "{} ways lost or invented money", ways);
        prop_assert_eq!(parts.len(), ways as usize);
    }

    #[test]
    fn no_two_shares_differ_by_more_than_one_minor_unit(
        total in 0_i64..100_000_000,
        ways in 1_u32..40,
    ) {
        // "Split evenly" has to look even to the people paying. A remainder spread anywhere but one
        // unit at a time is a split someone at the table will query.
        let parts = evenly(bdt(total), ways).expect("splits");
        let amounts: Vec<i64> = parts.iter().map(|part| part.amount.minor()).collect();
        let smallest = amounts.iter().min().copied().unwrap_or_default();
        let largest = amounts.iter().max().copied().unwrap_or_default();

        prop_assert!(largest - smallest <= 1, "shares ranged {}..{}", smallest, largest);
    }

    #[test]
    fn a_negative_bill_splits_the_same_way(
        total in -100_000_000_i64..0,
        ways in 1_u32..12,
    ) {
        // A refund is a negative bill, and it is split across the same people who paid.
        let parts = evenly(bdt(total), ways).expect("splits");
        let summed: i64 = parts.iter().map(|part| part.amount.minor()).sum();
        prop_assert_eq!(summed, total);
    }

    #[test]
    fn an_item_split_always_sums_to_the_bill(
        line_totals in prop::collection::vec(0_i64..1_000_000, 1..12),
        parts_wanted in 1_usize..5,
    ) {
        // Deal the lines round-robin across the parts. Every line lands exactly once, which is the
        // condition the function enforces and the one that makes the sum hold.
        let lines: Vec<SaleLine> = (0..line_totals.len())
            .map(|n| line(u128::try_from(n).unwrap_or_default() + 1))
            .collect();
        let totals: Vec<Money> = line_totals.iter().map(|minor| bdt(*minor)).collect();

        let mut assignment: Vec<Vec<Uuid>> = vec![Vec::new(); parts_wanted];
        for (index, line) in lines.iter().enumerate() {
            assignment[index % parts_wanted].push(line.id);
        }

        let parts = by_lines(&lines, &totals, &assignment).expect("splits");
        let summed: i64 = parts.iter().map(|part| part.amount.minor()).sum();
        let expected: i64 = line_totals.iter().sum();

        prop_assert_eq!(summed, expected);
    }

    #[test]
    fn every_line_is_charged_to_exactly_one_part(
        line_count in 1_usize..12,
        parts_wanted in 1_usize..5,
    ) {
        let lines: Vec<SaleLine> = (0..line_count).map(|n| line(u128::try_from(n).unwrap_or_default() + 1)).collect();
        let totals: Vec<Money> = (0..line_count)
            .map(|n| bdt(i64::try_from(n).unwrap_or_default() * 137 + 1))
            .collect();

        let mut assignment: Vec<Vec<Uuid>> = vec![Vec::new(); parts_wanted];
        for (index, line) in lines.iter().enumerate() {
            assignment[index % parts_wanted].push(line.id);
        }

        let parts = by_lines(&lines, &totals, &assignment).expect("splits");
        let charged: usize = parts.iter().map(|part| part.line_ids.len()).sum();

        prop_assert_eq!(charged, line_count, "a line was dropped or double-charged");
    }

    #[test]
    fn dropping_any_line_from_the_assignment_is_refused(
        line_count in 2_usize..8,
        drop_index in 0_usize..8,
    ) {
        // The failure this guards is silent: a line charged to nobody under-collects, and nobody
        // notices until the drawer is counted.
        let lines: Vec<SaleLine> = (0..line_count).map(|n| line(u128::try_from(n).unwrap_or_default() + 1)).collect();
        let totals: Vec<Money> = (0..line_count).map(|_| bdt(10_000)).collect();
        let dropped = drop_index % line_count;

        let assignment: Vec<Vec<Uuid>> = vec![
            lines
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != dropped)
                .map(|(_, line)| line.id)
                .collect(),
        ];

        prop_assert!(by_lines(&lines, &totals, &assignment).is_err());
    }
}
