//! An order becoming stock, and a transfer losing some on the way.
//!
//! These tests exist for the join. A purchase order and the batch ledger are separate models that
//! must agree — the document says what should arrive, the ledger says what did, and the only place
//! that can go wrong is between them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use sahl_core::Timestamp;
use sahl_core::inventory::{InventoryBook, InventoryEvent, IssueReason, total_on_hand};
use sahl_core::money::{Currency, Money};
use sahl_core::purchasing::{
    CloseReason, DispatchLine, OrderLine, OrderStatus, PurchaseEvent, PurchaseOrder, Transfer,
    TransferEvent, TransferStatus,
};
use sahl_core::quantity::Quantity;
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;
const DAY: i64 = 86_400_000;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn day(n: i64) -> Timestamp {
    Timestamp::from_millis(1_753_000_000_000 + n * DAY)
}

fn qty(milli: i64) -> Quantity {
    Quantity::from_milli(milli)
}

fn bdt(minor: i64) -> Money {
    Money::from_minor(minor, BDT)
}

const RICE: u128 = 0x21;
const OIL: u128 = 0x22;
const STAFF: u128 = 0xCA;
const DHANMONDI: u128 = 0xD1;
const GULSHAN: u128 = 0x62;

#[test]
fn an_order_becomes_stock_on_the_shelf() {
    // Two lines ordered, both delivered, both becoming batches. The document's received quantity
    // and the ledger's on-hand must be the same number.
    let order_events = vec![
        PurchaseEvent::Placed {
            order_id: id(1),
            supplier: "Karim Traders".to_owned(),
            reference: Some("KT-4471".to_owned()),
            lines: vec![
                OrderLine {
                    line_id: id(10),
                    product_id: id(RICE),
                    quantity: qty(50_000),
                    unit_cost: bdt(4_000),
                },
                OrderLine {
                    line_id: id(11),
                    product_id: id(OIL),
                    quantity: qty(12_000),
                    unit_cost: bdt(18_000),
                },
            ],
            expected_at: Some(day(3)),
            at: day(0),
            placed_by: id(STAFF),
        },
        PurchaseEvent::LineReceived {
            order_id: id(1),
            line_id: id(10),
            batch_id: id(0x901),
            quantity: qty(50_000),
            unit_cost: bdt(4_000),
            at: day(3),
            received_by: id(STAFF),
        },
        PurchaseEvent::LineReceived {
            order_id: id(1),
            line_id: id(11),
            batch_id: id(0x902),
            quantity: qty(12_000),
            unit_cost: bdt(18_000),
            at: day(3),
            received_by: id(STAFF),
        },
    ];

    let order = PurchaseOrder::replay(&order_events).expect("valid order");
    assert_eq!(order.status(), Ok(OrderStatus::FullyReceived));
    assert_eq!(
        order.received_value(),
        Ok(order.ordered_value().expect("computes"))
    );

    // The receipts, as the ledger sees them.
    let book = InventoryBook::replay(&[
        InventoryEvent::BatchReceived {
            batch_id: id(0x901),
            product_id: id(RICE),
            lot: Some("KT-4471/1".to_owned()),
            expires_at: Some(day(180)),
            quantity: qty(50_000),
            unit_cost: bdt(4_000),
            supplier: Some("Karim Traders".to_owned()),
            at: day(3),
            received_by: id(STAFF),
        },
        InventoryEvent::BatchReceived {
            batch_id: id(0x902),
            product_id: id(OIL),
            lot: Some("KT-4471/2".to_owned()),
            expires_at: Some(day(365)),
            quantity: qty(12_000),
            unit_cost: bdt(18_000),
            supplier: Some("Karim Traders".to_owned()),
            at: day(3),
            received_by: id(STAFF),
        },
    ])
    .expect("valid book");

    assert_eq!(total_on_hand(&book.levels()), Ok(qty(62_000)));
    assert_eq!(
        book.level(id(0x901)).expect("present").on_hand,
        order.line(id(10)).expect("present").received,
        "the document and the shelf agree"
    );
}

#[test]
fn a_short_delivery_leaves_a_gap_the_ledger_alone_cannot_see() {
    // Fifty kilos ordered, thirty arrived. The batch ledger records a perfectly consistent 30kg and
    // has no way to know 20 are missing — that is the entire reason the order document exists.
    let order = PurchaseOrder::replay(&[
        PurchaseEvent::Placed {
            order_id: id(1),
            supplier: "Karim Traders".to_owned(),
            reference: None,
            lines: vec![OrderLine {
                line_id: id(10),
                product_id: id(RICE),
                quantity: qty(50_000),
                unit_cost: bdt(4_000),
            }],
            expected_at: Some(day(3)),
            at: day(0),
            placed_by: id(STAFF),
        },
        PurchaseEvent::LineReceived {
            order_id: id(1),
            line_id: id(10),
            batch_id: id(0x901),
            quantity: qty(30_000),
            unit_cost: bdt(4_000),
            at: day(3),
            received_by: id(STAFF),
        },
    ])
    .expect("valid order");

    let book = InventoryBook::replay(&[InventoryEvent::BatchReceived {
        batch_id: id(0x901),
        product_id: id(RICE),
        lot: None,
        expires_at: None,
        quantity: qty(30_000),
        unit_cost: bdt(4_000),
        supplier: Some("Karim Traders".to_owned()),
        at: day(3),
        received_by: id(STAFF),
    }])
    .expect("valid book");

    assert!(book.variances().is_empty(), "the ledger sees nothing wrong");
    assert!(book.negative_batches().is_empty());

    assert_eq!(order.status(), Ok(OrderStatus::PartlyReceived));
    assert_eq!(order.outstanding().expect("computes").len(), 1);
    assert_eq!(
        order.line(id(10)).expect("present").outstanding(),
        Ok(qty(20_000)),
        "20kg short, visible only against the order"
    );
    assert_eq!(order.received_value(), Ok(bdt(120_000)));
    assert_eq!(order.ordered_value(), Ok(bdt(200_000)));
}

#[test]
fn a_supplier_raising_a_price_between_quote_and_delivery_is_caught() {
    let order = PurchaseOrder::replay(&[
        PurchaseEvent::Placed {
            order_id: id(1),
            supplier: "Karim Traders".to_owned(),
            reference: None,
            lines: vec![OrderLine {
                line_id: id(10),
                product_id: id(RICE),
                quantity: qty(50_000),
                unit_cost: bdt(4_000),
            }],
            expected_at: None,
            at: day(0),
            placed_by: id(STAFF),
        },
        PurchaseEvent::LineReceived {
            order_id: id(1),
            line_id: id(10),
            batch_id: id(0x901),
            quantity: qty(50_000),
            unit_cost: bdt(4_600),
            at: day(3),
            received_by: id(STAFF),
        },
    ])
    .expect("valid order");

    let flagged = order.price_discrepancies().expect("computes");
    assert_eq!(flagged.len(), 1);
    assert_eq!(order.ordered_value(), Ok(bdt(200_000)));
    assert_eq!(order.received_value(), Ok(bdt(230_000)), "300 taka more");
}

#[test]
fn stock_in_transit_belongs_to_neither_outlet() {
    // Ten crates leave Dhanmondi. Until Gulshan records them arriving, the total across both
    // outlets is genuinely short — and pretending otherwise makes one outlet's count wrong for as
    // long as the van is on the road.
    let dispatch = TransferEvent::Dispatched {
        transfer_id: id(1),
        from_outlet: id(DHANMONDI),
        to_outlet: id(GULSHAN),
        lines: vec![DispatchLine {
            line_id: id(10),
            product_id: id(RICE),
            batch_id: id(0x901),
            quantity: qty(10_000),
        }],
        at: day(0),
        dispatched_by: id(STAFF),
    };

    let transfer = Transfer::replay(&[dispatch]).expect("valid transfer");
    assert_eq!(transfer.status(), Ok(TransferStatus::InTransit));

    // Dhanmondi's ledger: received 25kg, sent 10kg out.
    let sender = InventoryBook::replay(&[
        InventoryEvent::BatchReceived {
            batch_id: id(0x901),
            product_id: id(RICE),
            lot: None,
            expires_at: None,
            quantity: qty(25_000),
            unit_cost: bdt(4_000),
            supplier: None,
            at: day(-1),
            received_by: id(STAFF),
        },
        InventoryEvent::StockIssued {
            batch_id: id(0x901),
            quantity: qty(10_000),
            reason: IssueReason::TransferOut,
            sale_id: None,
            at: day(0),
            issued_by: id(STAFF),
        },
    ])
    .expect("valid book");

    assert_eq!(total_on_hand(&sender.levels()), Ok(qty(15_000)));
    assert_eq!(
        transfer.in_transit().expect("computes")[0].1,
        qty(10_000),
        "the missing 10kg is accounted for by the transfer, not lost"
    );
}

#[test]
fn a_transfer_that_arrives_short_records_the_loss() {
    // Ten crates left, nine arrived. Settling must not quietly reconcile the tenth away — that gap
    // is the number a two-outlet owner is looking for.
    let transfer = Transfer::replay(&[
        TransferEvent::Dispatched {
            transfer_id: id(1),
            from_outlet: id(DHANMONDI),
            to_outlet: id(GULSHAN),
            lines: vec![DispatchLine {
                line_id: id(10),
                product_id: id(RICE),
                batch_id: id(0x901),
                quantity: qty(10_000),
            }],
            at: day(0),
            dispatched_by: id(STAFF),
        },
        TransferEvent::Received {
            transfer_id: id(1),
            line_id: id(10),
            batch_id: id(0xA01),
            quantity: qty(9_000),
            at: day(1),
            received_by: id(STAFF),
        },
        TransferEvent::Settled {
            transfer_id: id(1),
            at: day(2),
            settled_by: id(STAFF),
        },
    ])
    .expect("valid transfer");

    assert_eq!(
        transfer.status(),
        Ok(TransferStatus::Settled { short: true })
    );
    assert_eq!(transfer.in_transit().expect("computes")[0].1, qty(1_000));

    // Gulshan's ledger holds a new batch id: it is a different outlet's stock now, and reusing the
    // sender's would make one batch appear to be in two places at once.
    let receiver = InventoryBook::replay(&[InventoryEvent::BatchReceived {
        batch_id: id(0xA01),
        product_id: id(RICE),
        lot: None,
        expires_at: None,
        quantity: qty(9_000),
        unit_cost: bdt(4_000),
        supplier: None,
        at: day(1),
        received_by: id(STAFF),
    }])
    .expect("valid book");

    assert_eq!(total_on_hand(&receiver.levels()), Ok(qty(9_000)));
    assert!(receiver.level(id(0x901)).is_none());
}

#[test]
fn closing_a_short_order_settles_the_expectation_without_inventing_stock() {
    let order = PurchaseOrder::replay(&[
        PurchaseEvent::Placed {
            order_id: id(1),
            supplier: "Karim Traders".to_owned(),
            reference: None,
            lines: vec![OrderLine {
                line_id: id(10),
                product_id: id(RICE),
                quantity: qty(50_000),
                unit_cost: bdt(4_000),
            }],
            expected_at: None,
            at: day(0),
            placed_by: id(STAFF),
        },
        PurchaseEvent::LineReceived {
            order_id: id(1),
            line_id: id(10),
            batch_id: id(0x901),
            quantity: qty(30_000),
            unit_cost: bdt(4_000),
            at: day(3),
            received_by: id(STAFF),
        },
        PurchaseEvent::Closed {
            order_id: id(1),
            reason: CloseReason::ShortShipped,
            at: day(10),
            closed_by: id(STAFF),
        },
    ])
    .expect("valid order");

    assert_eq!(
        order.status(),
        Ok(OrderStatus::Closed(CloseReason::ShortShipped))
    );
    assert_eq!(
        order.line(id(10)).expect("present").received,
        qty(30_000),
        "closing changes the expectation, not the stock"
    );
    assert_eq!(order.received_value(), Ok(bdt(120_000)));
}
