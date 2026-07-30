//! Shift reconciliation, exercised the way an evening close actually runs.
//!
//! These are the numbers a merchant checks against a physical drawer, so a mistake here is one they
//! find before we do.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use sahl_core::Timestamp;
use sahl_core::money::{Currency, Money, Rounding};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{Sale, SaleEvent, TenderMethod, VoidReason, Wallet};
use sahl_core::shift::{
    CashMovementReason, Shift, ShiftError, ShiftEvent, ShiftStatus, Variance, report,
};
use sahl_core::tax::{PricingMode, TaxClass};
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn bdt(minor: i64) -> Money {
    Money::from_minor(minor, BDT)
}

fn at(minutes: i64) -> Timestamp {
    Timestamp::from_millis(1_753_000_000_000 + minutes * 60 * 1_000)
}

const SHIFT: u128 = 0x5417;
const CASHIER: u128 = 0xCA51;
const MANAGER: u128 = 0x11A;

fn opened(float_minor: i64) -> ShiftEvent {
    ShiftEvent::Opened {
        shift_id: id(SHIFT),
        opened_by: id(CASHIER),
        currency: BDT,
        opening_float: bdt(float_minor),
        at: at(0),
    }
}

fn moved(minor: i64, reason: CashMovementReason, minute: i64) -> ShiftEvent {
    ShiftEvent::CashMoved {
        shift_id: id(SHIFT),
        movement_id: Uuid::from_u128(0x9000 + minor.unsigned_abs() as u128),
        amount: bdt(minor),
        reason,
        note: None,
        authorized_by: id(MANAGER),
        at: at(minute),
    }
}

fn counted(minor: i64, minute: i64) -> ShiftEvent {
    ShiftEvent::Counted {
        shift_id: id(SHIFT),
        counted: bdt(minor),
        counted_by: id(CASHIER),
        at: at(minute),
    }
}

fn closed(minor: i64, minute: i64) -> ShiftEvent {
    ShiftEvent::Closed {
        shift_id: id(SHIFT),
        closed_by: id(CASHIER),
        closing_cash: bdt(minor),
        at: at(minute),
    }
}

/// One completed sale: `tendered` handed over for a `total` basket.
fn sale(n: u128, total: i64, tendered: i64, method: TenderMethod, minute: i64) -> Sale {
    let sale_id = id(0x5A00 + n);
    Sale::replay(&[
        SaleEvent::Opened {
            sale_id,
            opened_by: id(CASHIER),
            currency: BDT,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        },
        SaleEvent::LineAdded {
            sale_id,
            line_id: id(0x5B00 + n),
            product_id: id(0x5C00 + n),
            name: format!("Item {n}"),
            unit_price: bdt(total),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            modifiers: Vec::new(),
        },
        SaleEvent::TenderRecorded {
            sale_id,
            tender_id: id(0x5D00 + n),
            method,
            amount: bdt(tendered),
            reference: None,
        },
        SaleEvent::Completed {
            sale_id,
            total: bdt(total),
            change_given: bdt((tendered - total).max(0)),
            at: at(minute),
        },
    ])
    .expect("valid sale")
}

#[test]
fn an_empty_shift_expects_exactly_its_float() {
    let shift = Shift::replay(&[opened(500_000)]).expect("valid");
    let summary = report(&shift, &[]).expect("report");

    assert_eq!(summary.expected_cash, bdt(500_000));
    assert_eq!(summary.takings, bdt(0));
    assert!(!summary.is_final, "an X report leaves the session running");
}

#[test]
fn cash_sales_add_only_what_stayed_in_the_drawer() {
    // Over-tendering does not put more in the till; the change came straight back out.
    let shift = Shift::replay(&[opened(500_000)]).expect("valid");
    let sales = vec![
        sale(1, 48_000, 50_000, TenderMethod::Cash, 10),
        sale(2, 34_000, 34_000, TenderMethod::Cash, 20),
    ];

    let summary = report(&shift, &sales).expect("report");

    assert_eq!(summary.takings, bdt(82_000));
    assert_eq!(
        summary.cash_from_sales,
        bdt(82_000),
        "change is not takings"
    );
    assert_eq!(summary.expected_cash, bdt(582_000));
}

#[test]
fn card_and_wallet_takings_never_reach_the_drawer() {
    // The classic way a shift report looks short every single day.
    let shift = Shift::replay(&[opened(500_000)]).expect("valid");
    let sales = vec![
        sale(1, 48_000, 48_000, TenderMethod::Card, 10),
        sale(
            2,
            30_000,
            30_000,
            TenderMethod::MobileWallet {
                wallet: Wallet::Bkash,
            },
            15,
        ),
        sale(3, 20_000, 20_000, TenderMethod::Cash, 20),
    ];

    let summary = report(&shift, &sales).expect("report");

    assert_eq!(summary.takings, bdt(98_000), "all tenders count as takings");
    assert_eq!(
        summary.cash_from_sales,
        bdt(20_000),
        "only cash reaches the till"
    );
    assert_eq!(summary.expected_cash, bdt(520_000));
}

#[test]
fn a_skim_reduces_what_the_drawer_should_hold() {
    let shift = Shift::replay(&[
        opened(500_000),
        moved(-200_000, CashMovementReason::Skim, 30),
    ])
    .expect("valid");
    let sales = vec![sale(1, 48_000, 48_000, TenderMethod::Cash, 10)];

    let summary = report(&shift, &sales).expect("report");

    assert_eq!(summary.net_movements, bdt(-200_000));
    assert_eq!(summary.expected_cash, bdt(348_000));
}

#[test]
fn a_balanced_drawer_reports_no_variance() {
    let shift = Shift::replay(&[opened(500_000), counted(548_000, 60)]).expect("valid");
    let sales = vec![sale(1, 48_000, 48_000, TenderMethod::Cash, 10)];

    let summary = report(&shift, &sales).expect("report");

    assert_eq!(summary.counted_cash, Some(bdt(548_000)));
    assert_eq!(summary.variance, Some(Variance::Balanced));
    assert!(summary.variance.expect("some").is_balanced());
}

#[test]
fn a_short_drawer_names_the_shortfall() {
    // The number an owner acts on.
    let shift = Shift::replay(&[opened(500_000), counted(543_000, 60)]).expect("valid");
    let sales = vec![sale(1, 48_000, 48_000, TenderMethod::Cash, 10)];

    let summary = report(&shift, &sales).expect("report");

    assert_eq!(summary.variance, Some(Variance::Short { by: bdt(5_000) }));
}

#[test]
fn an_over_drawer_is_reported_too_because_it_is_not_good_news() {
    // A consistent over usually means sales are going unrecorded and the cash arrives anyway.
    let shift = Shift::replay(&[opened(500_000), counted(551_000, 60)]).expect("valid");
    let sales = vec![sale(1, 48_000, 48_000, TenderMethod::Cash, 10)];

    let summary = report(&shift, &sales).expect("report");

    assert_eq!(summary.variance, Some(Variance::Over { by: bdt(3_000) }));
    assert_eq!(summary.variance.expect("some").magnitude(), bdt(3_000));
}

#[test]
fn recounts_are_kept_and_counted() {
    // A cashier who counts twice, and whose second attempt suddenly matches, is worth seeing.
    let shift = Shift::replay(&[opened(500_000), counted(500_000, 58), counted(548_000, 59)])
        .expect("valid");
    let sales = vec![sale(1, 48_000, 48_000, TenderMethod::Cash, 10)];

    let summary = report(&shift, &sales).expect("report");

    assert_eq!(summary.count_attempts, 2);
    assert_eq!(
        summary.counted_cash,
        Some(bdt(548_000)),
        "the last count is the one the drawer closed on"
    );
    assert_eq!(shift.counts().len(), 2, "the first attempt is not erased");
}

#[test]
fn a_shift_cannot_close_without_a_count() {
    // Closing blind would make the variance unknowable, which is the whole point of the report.
    let result = Shift::replay(&[opened(500_000), closed(548_000, 60)]);
    assert_eq!(result, Err(ShiftError::NotCounted));
}

#[test]
fn a_closed_shift_is_immutable() {
    let result = Shift::replay(&[
        opened(500_000),
        counted(500_000, 59),
        closed(500_000, 60),
        moved(-100_000, CashMovementReason::Skim, 61),
    ]);
    assert_eq!(result, Err(ShiftError::Closed));
}

#[test]
fn a_negative_count_is_refused() {
    let result = Shift::replay(&[opened(500_000), counted(-1, 59)]);
    assert!(matches!(result, Err(ShiftError::NegativeCount { .. })));
}

#[test]
fn a_z_report_is_marked_final() {
    let shift = Shift::replay(&[opened(500_000), counted(500_000, 59), closed(500_000, 60)])
        .expect("valid");
    let summary = report(&shift, &[]).expect("report");

    assert!(summary.is_final);
    assert_eq!(shift.status(), ShiftStatus::Closed);
}

#[test]
fn sales_outside_the_shift_window_are_excluded() {
    // A report cannot be made to look better by handing it a curated list — membership is decided
    // here, from completion time.
    let shift = Shift::replay(&[opened(500_000), counted(548_000, 59), closed(548_000, 60)])
        .expect("valid");

    let sales = vec![
        sale(1, 48_000, 48_000, TenderMethod::Cash, 10),
        sale(2, 90_000, 90_000, TenderMethod::Cash, 120), // after close
    ];

    let summary = report(&shift, &sales).expect("report");

    assert_eq!(summary.sale_count, 1);
    assert_eq!(summary.takings, bdt(48_000));
    assert_eq!(summary.variance, Some(Variance::Balanced));
}

#[test]
fn a_sale_completing_before_the_shift_opened_is_excluded() {
    let shift = Shift::replay(&[opened(500_000)]).expect("valid");
    let sales = vec![sale(1, 48_000, 48_000, TenderMethod::Cash, -5)];

    let summary = report(&shift, &sales).expect("report");
    assert_eq!(summary.sale_count, 0);
}

#[test]
fn voids_across_the_shift_are_totalled_for_the_owner_feed() {
    let shift = Shift::replay(&[opened(500_000)]).expect("valid");

    let sale_id = id(0x7777);
    let with_void = Sale::replay(&[
        SaleEvent::Opened {
            sale_id,
            opened_by: id(CASHIER),
            currency: BDT,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        },
        SaleEvent::LineAdded {
            sale_id,
            line_id: id(1),
            product_id: id(2),
            name: "Kept".to_owned(),
            unit_price: bdt(10_000),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            modifiers: Vec::new(),
        },
        SaleEvent::LineAdded {
            sale_id,
            line_id: id(3),
            product_id: id(4),
            name: "Voided".to_owned(),
            unit_price: bdt(90_000),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            modifiers: Vec::new(),
        },
        SaleEvent::LineVoided {
            sale_id,
            line_id: id(3),
            reason: VoidReason::Mistake,
            authorized_by: id(MANAGER),
        },
        SaleEvent::TenderRecorded {
            sale_id,
            tender_id: id(5),
            method: TenderMethod::Cash,
            amount: bdt(10_000),
            reference: None,
        },
        SaleEvent::Completed {
            sale_id,
            total: bdt(10_000),
            change_given: bdt(0),
            at: at(10),
        },
    ])
    .expect("valid sale");

    let summary = report(&shift, std::slice::from_ref(&with_void)).expect("report");

    assert_eq!(summary.void_count, 1);
    assert_eq!(summary.takings, bdt(10_000), "the void does not sell");
}

#[test]
fn the_full_evening_reconciles() {
    // An ordinary close: float, a mixed run of sales, a mid-shift skim, a count that balances.
    let shift = Shift::replay(&[
        opened(500_000),
        moved(-300_000, CashMovementReason::Skim, 200),
        moved(50_000, CashMovementReason::FloatTopUp, 210),
        counted(432_000, 470),
        closed(432_000, 480),
    ])
    .expect("valid");

    let sales = vec![
        sale(1, 48_000, 50_000, TenderMethod::Cash, 30),
        sale(2, 34_000, 34_000, TenderMethod::Card, 60),
        sale(3, 100_000, 100_000, TenderMethod::Cash, 120),
        sale(4, 82_000, 100_000, TenderMethod::Cash, 300),
    ];

    let summary = report(&shift, &sales).expect("report");

    // 500,000 float − 300,000 skim + 50,000 top-up + (48,000 + 100,000 + 82,000) cash = 480,000.
    assert_eq!(summary.cash_from_sales, bdt(230_000));
    assert_eq!(summary.net_movements, bdt(-250_000));
    assert_eq!(summary.expected_cash, bdt(480_000));

    // Counted 432,000 — short by 48,000, exactly one sale's worth.
    assert_eq!(summary.variance, Some(Variance::Short { by: bdt(48_000) }));
    assert_eq!(summary.takings, bdt(264_000), "all four sales, all tenders");
    assert_eq!(summary.sale_count, 4);
    assert!(summary.is_final);
}
