//! Property tests for the VAT engine.
//!
//! These encode the invariants a fiscal auditor checks and a merchant notices: that an invoice's
//! summary agrees with its own lines, that a tax-inclusive shelf price rings up unchanged, and that
//! discounting can never manufacture money. Each holds for *every* generated basket, not just the
//! handful of examples in the unit tests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use proptest::prelude::*;
use sahl_core::tax::calculate;
use sahl_core::{
    Currency, Discount, LineInput, Money, OrderInput, PricingMode, Quantity, Rate, Rounding,
    TaxClass,
};

const BDT: Currency = Currency::Bdt;

/// The real Bangladeshi VAT ladder plus the two non-taxable classes.
fn tax_class() -> impl Strategy<Value = TaxClass> {
    prop_oneof![
        Just(TaxClass::standard(1500)),
        Just(TaxClass::standard(750)),
        Just(TaxClass::standard(500)),
        Just(TaxClass::standard(450)),
        Just(TaxClass::standard(240)),
        Just(TaxClass::ZeroRated),
        Just(TaxClass::Exempt),
    ]
}

fn line_discount() -> impl Strategy<Value = Discount> {
    prop_oneof![
        Just(Discount::None),
        (0i32..=5_000).prop_map(|bp| Discount::Percentage {
            rate: Rate::from_basis_points(bp)
        }),
        (0i64..=50_000).prop_map(|minor| Discount::Amount {
            amount: Money::from_minor(minor, BDT)
        }),
    ]
}

/// Prices and quantities in the range a real till sees: up to ৳100,000 a unit, up to 100 units,
/// including fractional weights.
fn line() -> impl Strategy<Value = LineInput> {
    (
        0i64..=10_000_000,
        1i64..=100_000,
        tax_class(),
        line_discount(),
    )
        .prop_map(|(price, milli, class, discount)| {
            LineInput::new(
                Money::from_minor(price, BDT),
                Quantity::from_milli(milli),
                class,
            )
            .with_discount(discount)
        })
}

fn order() -> impl Strategy<Value = OrderInput> {
    (
        prop::collection::vec(line(), 1..=20),
        prop_oneof![
            Just(PricingMode::TaxInclusive),
            Just(PricingMode::TaxExclusive)
        ],
        prop_oneof![
            Just(Discount::None),
            (0i32..=3_000).prop_map(|bp| Discount::Percentage {
                rate: Rate::from_basis_points(bp)
            }),
            (0i64..=100_000).prop_map(|minor| Discount::Amount {
                amount: Money::from_minor(minor, BDT)
            }),
        ],
        prop_oneof![
            Just(Rounding::HalfUp),
            Just(Rounding::HalfEven),
            Just(Rounding::TowardZero)
        ],
    )
        .prop_map(|(lines, mode, order_discount, rounding)| {
            let mut order = OrderInput::new(BDT, lines).with_order_discount(order_discount);
            order.pricing_mode = mode;
            order.rounding = rounding;
            order
        })
}

proptest! {
    /// An invoice whose summary disagrees with its own lines is the classic POS defect, and the
    /// first thing a fiscal auditor looks for. Every aggregate must be the exact sum of its parts.
    #[test]
    fn every_aggregate_is_the_exact_sum_of_its_lines(order in order()) {
        let totals = calculate(&order).expect("calculates");

        prop_assert_eq!(
            Money::try_sum(totals.lines.iter().map(|line| line.net), BDT).expect("no overflow"),
            totals.net
        );
        prop_assert_eq!(
            Money::try_sum(totals.lines.iter().map(|line| line.tax), BDT).expect("no overflow"),
            totals.tax
        );
        prop_assert_eq!(
            Money::try_sum(totals.lines.iter().map(|line| line.total), BDT).expect("no overflow"),
            totals.total
        );
        prop_assert_eq!(
            Money::try_sum(totals.lines.iter().map(|line| line.gross), BDT).expect("no overflow"),
            totals.gross
        );
    }

    /// `net + tax == total` at both line and order level, with no reconciliation step.
    #[test]
    fn net_and_tax_always_reconstruct_the_total(order in order()) {
        let totals = calculate(&order).expect("calculates");

        for line in &totals.lines {
            prop_assert_eq!(line.net.checked_add(line.tax).expect("no overflow"), line.total);
        }
        prop_assert_eq!(totals.net.checked_add(totals.tax).expect("no overflow"), totals.total);
    }

    /// The VAT summary block must account for every taka of tax and every taka of base.
    #[test]
    fn the_vat_summary_accounts_for_everything(order in order()) {
        let totals = calculate(&order).expect("calculates");

        prop_assert_eq!(
            Money::try_sum(totals.tax_groups.iter().map(|group| group.tax), BDT).expect("no overflow"),
            totals.tax
        );
        prop_assert_eq!(
            Money::try_sum(totals.tax_groups.iter().map(|group| group.taxable_base), BDT).expect("no overflow"),
            totals.net
        );
    }

    /// Each tax class appears exactly once in the summary, in stable invoice order — so the same
    /// basket produces a byte-identical document on the terminal and on the server.
    #[test]
    fn summary_groups_are_unique_and_stably_ordered(order in order()) {
        let totals = calculate(&order).expect("calculates");

        let keys: Vec<(u8, i32)> = totals
            .tax_groups
            .iter()
            .map(|group| group.tax_class.sort_key())
            .collect();

        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();

        prop_assert_eq!(keys, sorted, "groups must be sorted and free of duplicates");
    }

    /// **The shelf-label guarantee.** Under tax-inclusive pricing the customer pays exactly the
    /// quoted price less any discount — the tax comes out of it, never on top of it. A merchant
    /// notices a one-paisa deviation here on day one.
    #[test]
    fn tax_inclusive_pricing_preserves_the_quoted_price(order in order()) {
        prop_assume!(order.pricing_mode == PricingMode::TaxInclusive);
        let totals = calculate(&order).expect("calculates");

        prop_assert_eq!(
            totals.gross.checked_sub(totals.discount).expect("no overflow"),
            totals.total
        );
    }

    /// Under tax-exclusive pricing the net is the discounted gross and tax is added on top.
    #[test]
    fn tax_exclusive_pricing_adds_tax_on_top(order in order()) {
        prop_assume!(order.pricing_mode == PricingMode::TaxExclusive);
        let totals = calculate(&order).expect("calculates");

        prop_assert_eq!(
            totals.gross.checked_sub(totals.discount).expect("no overflow"),
            totals.net
        );
        prop_assert_eq!(
            totals.net.checked_add(totals.tax).expect("no overflow"),
            totals.total
        );
    }

    /// Non-taxable classes contribute no tax, whatever the price, discount, or rounding mode.
    #[test]
    fn non_taxable_classes_never_produce_tax(order in order()) {
        let totals = calculate(&order).expect("calculates");

        for line in &totals.lines {
            if !line.tax_class.is_taxable() {
                prop_assert!(line.tax.is_zero(), "{:?} produced tax {}", line.tax_class, line.tax);
            }
        }
    }

    /// Discounting can reduce a bill to zero but never below it — a cashier who can drive a total
    /// negative can make a drawer balance.
    #[test]
    fn discounts_can_never_drive_a_sale_negative(order in order()) {
        let totals = calculate(&order).expect("calculates");

        prop_assert!(!totals.total.is_negative(), "total went negative: {}", totals.total);
        prop_assert!(!totals.tax.is_negative(), "tax went negative: {}", totals.tax);
        prop_assert!(!totals.discount.is_negative());
        prop_assert!(totals.discount.minor() <= totals.gross.minor(), "discounted more than the basket");
    }

    /// Tax never exceeds the base it is charged on. Every rate in the generated ladder is well
    /// under 100%, so a line whose VAT matched or beat its own net value would mean the inclusive
    /// extraction or the exclusive application had inverted.
    #[test]
    fn tax_never_exceeds_its_own_base(order in order()) {
        let totals = calculate(&order).expect("calculates");

        for line in &totals.lines {
            prop_assert!(
                line.tax.minor() <= line.net.minor(),
                "line tax {} exceeds its net {} ({:?})", line.tax, line.net, line.tax_class
            );
        }
        prop_assert!(totals.tax.minor() <= totals.net.minor());
    }

    /// The same order calculated twice yields the same answer. Terminal and server both run this;
    /// any dependence on iteration or hash order would show up here.
    #[test]
    fn calculation_is_deterministic(order in order()) {
        prop_assert_eq!(calculate(&order).expect("calculates"), calculate(&order).expect("calculates"));
    }

    /// Applying an order discount never increases what the customer pays.
    #[test]
    fn an_order_discount_never_raises_the_total(
        lines in prop::collection::vec(line(), 1..=12),
        discount_minor in 1i64..=100_000,
    ) {
        let undiscounted = OrderInput::new(BDT, lines.clone());
        let discounted = OrderInput::new(BDT, lines).with_order_discount(Discount::Amount {
            amount: Money::from_minor(discount_minor, BDT),
        });

        let before = calculate(&undiscounted).expect("calculates");
        let after = calculate(&discounted).expect("calculates");

        prop_assert!(after.total.minor() <= before.total.minor());
    }

    /// An order-level discount is fully apportioned: the shares across lines sum to exactly the
    /// discount granted, so nothing is lost or invented by spreading it.
    #[test]
    fn an_order_discount_is_apportioned_without_loss(
        lines in prop::collection::vec(line(), 1..=12),
        discount_minor in 0i64..=50_000,
    ) {
        let plain = OrderInput::new(BDT, lines.clone());
        let discounted = OrderInput::new(BDT, lines).with_order_discount(Discount::Amount {
            amount: Money::from_minor(discount_minor, BDT),
        });

        let before = calculate(&plain).expect("calculates");
        let after = calculate(&discounted).expect("calculates");

        // Whatever the order discount actually resolved to (it is capped at the subtotal), the
        // difference in total discount must equal the difference in total charged.
        let extra_discount = after.discount.checked_sub(before.discount).expect("no overflow");
        let reduction = before.total.checked_sub(after.total).expect("no overflow");

        prop_assert_eq!(extra_discount, reduction);
    }
}
