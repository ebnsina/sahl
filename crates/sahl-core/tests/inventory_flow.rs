//! Inventory from delivery to shelf to count.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use sahl_core::Timestamp;
use sahl_core::inventory::{
    InventoryBook, InventoryError, InventoryEvent, IssueReason, ReturnReason, expired,
    expiring_soon, pick_fefo, sellable_on_hand, total_on_hand,
};
use sahl_core::money::{Currency, Money};
use sahl_core::quantity::Quantity;
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;
const DAY: i64 = 86_400_000;
const WEEK: i64 = DAY * 7;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn day(n: i64) -> Timestamp {
    Timestamp::from_millis(1_753_000_000_000 + n * DAY)
}

fn qty(milli: i64) -> Quantity {
    Quantity::from_milli(milli)
}

const RICE: u128 = 0x21;
const MILK: u128 = 0x22;
const STAFF: u128 = 0xCA51;

fn received(
    batch: u128,
    product: u128,
    expiry: Option<i64>,
    on: i64,
    milli: i64,
) -> InventoryEvent {
    InventoryEvent::BatchReceived {
        batch_id: id(batch),
        product_id: id(product),
        lot: Some(format!("LOT{batch}")),
        expires_at: expiry.map(day),
        quantity: qty(milli),
        unit_cost: Money::from_minor(4_000, BDT),
        supplier: Some("Karim Traders".to_owned()),
        at: day(on),
        received_by: id(STAFF),
    }
}

fn issued(batch: u128, milli: i64, reason: IssueReason, on: i64) -> InventoryEvent {
    InventoryEvent::StockIssued {
        batch_id: id(batch),
        quantity: qty(milli),
        reason,
        sale_id: Some(id(0x5A1E)),
        at: day(on),
        issued_by: id(STAFF),
    }
}

fn counted(batch: u128, milli: i64, on: i64) -> InventoryEvent {
    InventoryEvent::BatchCounted {
        batch_id: id(batch),
        counted: qty(milli),
        at: day(on),
        counted_by: id(STAFF),
    }
}

#[test]
fn a_delivery_becomes_a_batch_on_the_shelf() {
    let book = InventoryBook::replay(&[received(1, RICE, Some(30), 0, 10_000)]).expect("valid");

    assert_eq!(book.level(id(1)).expect("present").on_hand, qty(10_000));
    assert_eq!(book.unit_cost(id(1)), Some(Money::from_minor(4_000, BDT)));
}

#[test]
fn a_second_delivery_is_a_separate_batch_not_a_top_up() {
    // Merging them is exactly what makes a recall under-report.
    let book = InventoryBook::replay(&[
        received(1, RICE, Some(30), 0, 10_000),
        received(2, RICE, Some(60), 5, 8_000),
    ])
    .expect("valid");

    assert_eq!(book.for_product(id(RICE)).len(), 2);
    assert_eq!(total_on_hand(&book.levels()), Ok(qty(18_000)));
}

#[test]
fn receiving_the_same_batch_twice_is_refused() {
    let result = InventoryBook::replay(&[
        received(1, RICE, Some(30), 0, 10_000),
        received(1, RICE, Some(30), 0, 10_000),
    ]);
    assert_eq!(
        result,
        Err(InventoryError::DuplicateBatch { batch_id: id(1) })
    );
}

#[test]
fn issuing_stock_draws_it_down() {
    let book = InventoryBook::replay(&[
        received(1, RICE, Some(30), 0, 10_000),
        issued(1, 3_000, IssueReason::Sale, 1),
    ])
    .expect("valid");

    assert_eq!(book.level(id(1)).expect("present").on_hand, qty(7_000));
}

#[test]
fn a_refund_puts_stock_back() {
    let book = InventoryBook::replay(&[
        received(1, RICE, Some(30), 0, 10_000),
        issued(1, 3_000, IssueReason::Sale, 1),
        InventoryEvent::StockReturned {
            batch_id: id(1),
            quantity: qty(1_000),
            reason: ReturnReason::CustomerRefund,
            sale_id: Some(id(0x5A1E)),
            at: day(2),
            returned_by: id(STAFF),
        },
    ])
    .expect("valid");

    assert_eq!(book.level(id(1)).expect("present").on_hand, qty(8_000));
}

#[test]
fn issuing_against_an_unreceived_batch_is_refused() {
    let result = InventoryBook::replay(&[issued(1, 1_000, IssueReason::Sale, 0)]);
    assert_eq!(
        result,
        Err(InventoryError::UnknownBatch { batch_id: id(1) })
    );
}

#[test]
fn a_zero_movement_is_refused() {
    let result = InventoryBook::replay(&[
        received(1, RICE, Some(30), 0, 10_000),
        issued(1, 0, IssueReason::Sale, 1),
    ]);
    assert!(matches!(
        result,
        Err(InventoryError::NonPositiveMovement { .. })
    ));
}

#[test]
fn stock_may_go_negative_and_that_is_the_signal() {
    // The shelf is the authority. A book that refuses to record what a shopkeeper physically did
    // is a book they stop maintaining.
    let book = InventoryBook::replay(&[
        received(1, RICE, Some(30), 0, 2_000),
        issued(1, 5_000, IssueReason::Sale, 1),
    ])
    .expect("valid");

    assert_eq!(book.level(id(1)).expect("present").on_hand, qty(-3_000));
    assert_eq!(book.negative_batches().len(), 1);
}

#[test]
fn a_count_sets_the_level_and_records_the_disagreement() {
    // The count wins, but the variance survives — overwriting silently would erase the only
    // evidence that stock went missing.
    let book = InventoryBook::replay(&[
        received(1, RICE, Some(30), 0, 10_000),
        issued(1, 3_000, IssueReason::Sale, 1),
        counted(1, 6_500, 2),
    ])
    .expect("valid");

    assert_eq!(book.level(id(1)).expect("present").on_hand, qty(6_500));
    assert_eq!(book.variances().len(), 1);

    let variance = book.variances()[0];
    assert_eq!(variance.expected, qty(7_000));
    assert_eq!(variance.counted, qty(6_500));
    assert_eq!(variance.delta, qty(-500), "500g missing");
}

#[test]
fn a_count_that_agrees_records_no_variance() {
    let book = InventoryBook::replay(&[
        received(1, RICE, Some(30), 0, 10_000),
        counted(1, 10_000, 1),
    ])
    .expect("valid");

    assert!(book.variances().is_empty());
}

#[test]
fn a_count_can_reveal_more_stock_than_expected() {
    // Over is worth recording too: a delivery that was never entered looks exactly like this.
    let book = InventoryBook::replay(&[
        received(1, RICE, Some(30), 0, 10_000),
        counted(1, 12_000, 1),
    ])
    .expect("valid");

    assert_eq!(book.variances()[0].delta, qty(2_000));
}

#[test]
fn a_count_clears_a_negative_level() {
    let book = InventoryBook::replay(&[
        received(1, RICE, Some(30), 0, 2_000),
        issued(1, 5_000, IssueReason::Sale, 1),
        counted(1, 500, 2),
    ])
    .expect("valid");

    assert!(book.negative_batches().is_empty());
    assert_eq!(book.variances()[0].delta, qty(3_500));
}

#[test]
fn a_negative_count_is_refused() {
    let result = InventoryBook::replay(&[received(1, RICE, Some(30), 0, 2_000), counted(1, -1, 1)]);
    assert!(matches!(result, Err(InventoryError::NegativeCount { .. })));
}

#[test]
fn repeated_shortfalls_on_one_batch_accumulate_into_a_record() {
    // One batch off a little is noise; the same batch off every count is what an owner wants.
    let book = InventoryBook::replay(&[
        received(1, RICE, Some(90), 0, 10_000),
        counted(1, 9_500, 1),
        counted(1, 9_000, 2),
        counted(1, 8_400, 3),
    ])
    .expect("valid");

    assert_eq!(book.variances().len(), 3);
    assert!(
        book.variances()
            .iter()
            .all(|variance| variance.delta.is_negative()),
        "shrinkage, every time"
    );
}

#[test]
fn the_whole_loop_runs_from_delivery_to_pick_to_count() {
    // Two deliveries, one expiring sooner. A sale should draw the soonest first, and the count
    // afterwards should reconcile against what is left.
    let mut book = InventoryBook::replay(&[
        received(1, MILK, Some(30), 0, 6_000),
        received(2, MILK, Some(3), 1, 4_000),
    ])
    .expect("valid");

    let levels = book.for_product(id(MILK));
    let pick = pick_fefo(&levels, qty(5_000), day(2)).expect("picks");

    assert!(pick.is_complete());
    assert_eq!(pick.allocations[0].batch_id, id(2), "soonest expiry first");
    assert_eq!(pick.allocations[0].taken, qty(4_000));
    assert_eq!(pick.allocations[1].taken, qty(1_000));

    // Record what the pick decided.
    for allocation in &pick.allocations {
        book.apply(&InventoryEvent::StockIssued {
            batch_id: allocation.batch_id,
            quantity: allocation.taken,
            reason: IssueReason::Sale,
            sale_id: Some(id(0x5A1E)),
            at: day(2),
            issued_by: id(STAFF),
        })
        .expect("applies");
    }

    assert_eq!(book.level(id(2)).expect("present").on_hand, qty(0));
    assert_eq!(book.level(id(1)).expect("present").on_hand, qty(5_000));

    // Day four: batch 2 is empty and past its date, batch 1 is fine.
    let levels = book.levels();
    assert!(
        expired(&levels, day(4)).is_empty(),
        "an empty batch is not a write-off"
    );
    assert_eq!(sellable_on_hand(&levels, day(4)), Ok(qty(5_000)));
}

#[test]
fn the_discount_list_finds_stock_before_it_is_lost() {
    let book = InventoryBook::replay(&[
        received(1, MILK, Some(3), 0, 4_000),
        received(2, MILK, Some(60), 0, 4_000),
        received(3, RICE, None, 0, 9_000),
    ])
    .expect("valid");

    let levels = book.levels();
    let soon = expiring_soon(&levels, day(0), WEEK);

    assert_eq!(soon.len(), 1);
    assert_eq!(soon[0].batch.id, id(1));
}

#[test]
fn replay_is_deterministic() {
    // The terminal and the server both do this and must agree on every level.
    let events = vec![
        received(1, MILK, Some(30), 0, 6_000),
        received(2, MILK, Some(3), 1, 4_000),
        issued(2, 1_000, IssueReason::Sale, 2),
        issued(1, 500, IssueReason::Wastage, 2),
        counted(1, 5_400, 3),
    ];

    assert_eq!(
        InventoryBook::replay(&events).expect("valid"),
        InventoryBook::replay(&events).expect("valid")
    );
}
