//! Replay determinism.
//!
//! The plan commits to this as a CI gate, and it is the load-bearing assumption behind the whole
//! architecture: the terminal and the server replay the same events and must reach byte-identical
//! results. If they can diverge, a merchant has two versions of their day and no way to tell which
//! one is right — and every downstream claim about auditability collapses with it.
//!
//! These generate whole plausible trading sessions — several tickets open at once, voids, discounts,
//! split payments, closes out of order — and assert the projection is a pure function of the event
//! stream.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use proptest::prelude::*;
use sahl_core::money::{Currency, Money, Rounding};
use sahl_core::projection::SaleBook;
use sahl_core::quantity::Quantity;
use sahl_core::sale::{SaleEvent, TenderMethod, VoidReason, Wallet};
use sahl_core::tax::{Discount, PricingMode, TaxClass};
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn bdt(minor: i64) -> Money {
    Money::from_minor(minor, BDT)
}

/// One ticket's worth of activity: items, an optional void, an optional discount, then payment.
#[derive(Debug, Clone)]
struct Ticket {
    prices: Vec<i64>,
    quantities: Vec<i64>,
    void_line: Option<usize>,
    order_discount_minor: i64,
    wallet_split: bool,
}

fn ticket_strategy() -> impl Strategy<Value = Ticket> {
    (
        prop::collection::vec(100i64..=500_000, 1..=6),
        prop::collection::vec(250i64..=8_000, 1..=6),
        prop::option::of(0usize..6),
        0i64..=5_000,
        any::<bool>(),
    )
        .prop_map(
            |(prices, quantities, void_line, order_discount_minor, wallet_split)| Ticket {
                prices,
                quantities,
                void_line,
                order_discount_minor,
                wallet_split,
            },
        )
}

/// Turn a ticket into a valid event sequence, computing the payment from the real engine so the
/// stream is always internally consistent.
fn events_for(ticket: &Ticket, seq: u128) -> Vec<SaleEvent> {
    let sale = seq * 1_000;
    let mut events = vec![SaleEvent::Opened {
        sale_id: id(sale),
        opened_by: id(0xCA51),
        currency: BDT,
        pricing_mode: PricingMode::TaxInclusive,
        rounding: Rounding::HalfUp,
    }];

    let count = ticket.prices.len().min(ticket.quantities.len());
    for position in 0..count {
        events.push(SaleEvent::LineAdded {
            sale_id: id(sale),
            line_id: id(sale + position as u128 + 1),
            product_id: id(sale + position as u128 + 100),
            name: format!("Item {position}"),
            unit_price: bdt(ticket.prices[position]),
            quantity: Quantity::from_milli(ticket.quantities[position]),
            // A realistic mixed-rate basket: standard, reduced, and exempt goods together.
            tax_class: match position % 3 {
                0 => TaxClass::standard(1500),
                1 => TaxClass::standard(750),
                _ => TaxClass::Exempt,
            },
        });
    }

    if let Some(position) = ticket.void_line.filter(|position| *position < count) {
        events.push(SaleEvent::LineVoided {
            sale_id: id(sale),
            line_id: id(sale + position as u128 + 1),
            reason: VoidReason::Mistake,
            authorized_by: id(0x11A),
        });
    }

    // Every line voided leaves nothing to sell; leave the ticket open rather than build an
    // impossible completion.
    let all_voided = ticket.void_line.is_some_and(|position| position < count) && count == 1;
    if all_voided {
        return events;
    }

    if ticket.order_discount_minor > 0 {
        events.push(SaleEvent::OrderDiscounted {
            sale_id: id(sale),
            discount: Discount::Amount {
                amount: bdt(ticket.order_discount_minor),
            },
            authorized_by: id(0x11A),
        });
    }

    // Ask the same engine the product uses what this comes to, so the tender always settles exactly.
    let projected = SaleBook::replay(&events).expect("valid so far");
    let total = projected
        .get(id(sale))
        .expect("sale exists")
        .totals()
        .expect("has active lines")
        .total;

    // A discount can take a basket to exactly zero — a comped or staff sale. That is a real
    // scenario and the aggregate handles it: a zero balance is not "outstanding", so the sale
    // completes with no tender at all. Handing over nothing is not a payment, though, so no tender
    // event is emitted.
    if total.is_zero() {
        events.push(SaleEvent::Completed {
            sale_id: id(sale),
            total,
            change_given: bdt(0),
        });
        return events;
    }

    if ticket.wallet_split && total.minor() > 1 {
        let half = total.minor() / 2;
        events.push(SaleEvent::TenderRecorded {
            sale_id: id(sale),
            tender_id: id(sale + 800),
            method: TenderMethod::MobileWallet {
                wallet: Wallet::Bkash,
            },
            amount: bdt(half),
            reference: Some("TRX123".to_owned()),
        });
        events.push(SaleEvent::TenderRecorded {
            sale_id: id(sale),
            tender_id: id(sale + 801),
            method: TenderMethod::Cash,
            amount: bdt(total.minor() - half),
            reference: None,
        });
    } else {
        events.push(SaleEvent::TenderRecorded {
            sale_id: id(sale),
            tender_id: id(sale + 800),
            method: TenderMethod::Cash,
            amount: total,
            reference: None,
        });
    }

    events.push(SaleEvent::Completed {
        sale_id: id(sale),
        total,
        change_given: bdt(0),
    });
    events
}

/// A trading session: several tickets, interleaved so they overlap in time.
fn session_strategy() -> impl Strategy<Value = Vec<SaleEvent>> {
    prop::collection::vec(ticket_strategy(), 1..=6).prop_map(|tickets| {
        let streams: Vec<Vec<SaleEvent>> = tickets
            .iter()
            .enumerate()
            .map(|(position, ticket)| events_for(ticket, position as u128 + 1))
            .collect();

        // Round-robin the streams together. Each ticket's own events stay in order — which is the
        // real constraint — while tickets overlap, as they do on a café floor.
        let longest = streams.iter().map(Vec::len).max().unwrap_or(0);
        let mut interleaved = Vec::new();
        for step in 0..longest {
            for stream in &streams {
                if let Some(event) = stream.get(step) {
                    interleaved.push(event.clone());
                }
            }
        }
        interleaved
    })
}

proptest! {
    /// **The gate.** Replaying the same stream twice produces byte-identical output.
    #[test]
    fn replay_is_byte_identical(events in session_strategy()) {
        let first = SaleBook::replay(&events).expect("valid session");
        let second = SaleBook::replay(&events).expect("valid session");

        prop_assert_eq!(
            first.fingerprint().expect("canonical"),
            second.fingerprint().expect("canonical")
        );
    }

    /// Applying events one at a time — as the terminal does live — lands in exactly the same place
    /// as replaying the batch, which is what the server does on sync.
    #[test]
    fn incremental_application_matches_batch_replay(events in session_strategy()) {
        let batch = SaleBook::replay(&events).expect("valid session");

        let mut incremental = SaleBook::new();
        for event in &events {
            incremental.apply(event).expect("valid event");
        }

        prop_assert_eq!(
            batch.fingerprint().expect("canonical"),
            incremental.fingerprint().expect("canonical")
        );
    }

    /// Replaying a prefix and then the remainder equals replaying the whole thing. This is exactly
    /// what a terminal does when it resumes mid-shift after a crash.
    #[test]
    fn resuming_from_a_prefix_matches_a_full_replay(
        events in session_strategy(),
        split_seed: u64,
    ) {
        prop_assume!(!events.is_empty());
        let split = usize::try_from(split_seed % events.len() as u64).unwrap();

        let whole = SaleBook::replay(&events).expect("valid session");

        let mut resumed = SaleBook::replay(&events[..split]).expect("valid prefix");
        for event in &events[split..] {
            resumed.apply(event).expect("valid event");
        }

        prop_assert_eq!(
            whole.fingerprint().expect("canonical"),
            resumed.fingerprint().expect("canonical")
        );
    }

    /// Takings equal the sum of what each completed sale actually charged. A shift report that
    /// disagrees with its own receipts is the number a merchant will catch first.
    #[test]
    fn takings_reconcile_against_the_individual_sales(events in session_strategy()) {
        let book = SaleBook::replay(&events).expect("valid session");

        let summed = Money::try_sum(
            book.completed().filter_map(|sale| sale.settled_total()),
            BDT,
        ).expect("no overflow");

        prop_assert_eq!(book.takings(BDT).expect("no overflow"), summed);
    }

    /// Voided lines are never silently dropped from the record.
    #[test]
    fn voided_lines_survive_into_the_projection(events in session_strategy()) {
        let book = SaleBook::replay(&events).expect("valid session");

        let voids_in_stream = events
            .iter()
            .filter(|event| matches!(event, SaleEvent::LineVoided { .. }))
            .count();

        prop_assert_eq!(book.void_count(), voids_in_stream);
    }
}
