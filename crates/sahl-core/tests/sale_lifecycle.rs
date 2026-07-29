//! The sale lifecycle, exercised as a real till would.
//!
//! These are integration tests rather than unit tests because a sale's correctness is a property of
//! whole event *sequences*, not of any single transition. What matters is that a plausible run of
//! events produces the right money, and that an implausible one is refused.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use sahl_core::money::{Currency, Money, Rounding};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{Sale, SaleError, SaleEvent, SaleStatus, TenderMethod, VoidReason, Wallet};
use sahl_core::tax::{Discount, PricingMode, TaxClass};
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;
const VAT_15: TaxClass = TaxClass::standard(1500);

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn bdt(minor: i64) -> Money {
    Money::from_minor(minor, BDT)
}

const SALE: u128 = 0x5A1E;
const CASHIER: u128 = 0xCA51;

fn opened() -> SaleEvent {
    SaleEvent::Opened {
        sale_id: id(SALE),
        opened_by: id(CASHIER),
        currency: BDT,
        pricing_mode: PricingMode::TaxInclusive,
        rounding: Rounding::HalfUp,
    }
}

fn line(line_id: u128, name: &str, unit_minor: i64, qty_milli: i64) -> SaleEvent {
    SaleEvent::LineAdded {
        sale_id: id(SALE),
        line_id: id(line_id),
        product_id: id(line_id + 1000),
        name: name.to_owned(),
        unit_price: bdt(unit_minor),
        quantity: Quantity::from_milli(qty_milli),
        tax_class: VAT_15,
    }
}

fn tender(method: TenderMethod, minor: i64) -> SaleEvent {
    SaleEvent::TenderRecorded {
        sale_id: id(SALE),
        tender_id: id(0x7E0 + u128::from(minor.unsigned_abs() % 1000)),
        method,
        amount: bdt(minor),
        reference: None,
    }
}

fn completed(total: i64, change: i64) -> SaleEvent {
    SaleEvent::Completed {
        sale_id: id(SALE),
        total: bdt(total),
        change_given: bdt(change),
        at: Timestamp::from_millis(1_753_000_000_000),
    }
}

#[test]
fn a_simple_cash_sale_settles_exactly() {
    // Two items at tax-inclusive prices: the customer pays exactly what the shelf said.
    let sale = Sale::replay(&[
        opened(),
        line(1, "Rice 5kg", 48_000, 1_000),
        line(2, "Cooking oil 2L", 34_000, 1_000),
        tender(TenderMethod::Cash, 82_000),
        completed(82_000, 0),
    ])
    .expect("valid sequence");

    assert_eq!(sale.status(), SaleStatus::Completed);
    assert_eq!(sale.settled_total(), Some(bdt(82_000)));
    assert_eq!(sale.change_given(), Some(bdt(0)));
    assert_eq!(sale.net_cash(), Ok(bdt(82_000)));
}

#[test]
fn change_is_derived_not_trusted() {
    let sale = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        tender(TenderMethod::Cash, 10_000),
        completed(5_500, 4_500),
    ])
    .expect("valid sequence");

    assert_eq!(sale.change_given(), Some(bdt(4_500)));
    // The drawer keeps only what was actually kept.
    assert_eq!(sale.net_cash(), Ok(bdt(5_500)));
}

#[test]
fn a_wrong_change_figure_is_refused() {
    // A terminal claiming it gave less change than the arithmetic says is either broken or skimming.
    let result = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        tender(TenderMethod::Cash, 10_000),
        completed(5_500, 3_000),
    ]);

    assert_eq!(
        result,
        Err(SaleError::ChangeMismatch {
            recorded: bdt(3_000),
            calculated: bdt(4_500),
        })
    );
}

#[test]
fn a_wrong_total_is_refused() {
    // The recorded total is recomputed, not believed. A mismatch means tampering or version skew.
    let result = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        tender(TenderMethod::Cash, 5_000),
        completed(5_000, 0),
    ]);

    assert!(matches!(result, Err(SaleError::TotalMismatch { .. })));
}

#[test]
fn a_sale_cannot_close_while_money_is_outstanding() {
    let result = Sale::replay(&[
        opened(),
        line(1, "Rice 5kg", 48_000, 1_000),
        tender(TenderMethod::Cash, 20_000),
        completed(48_000, 0),
    ]);

    assert_eq!(
        result,
        Err(SaleError::Outstanding {
            outstanding: bdt(28_000)
        })
    );
}

#[test]
fn a_split_payment_across_cash_and_wallet_settles() {
    // Entirely ordinary in the launch market: part bKash, part cash.
    let sale = Sale::replay(&[
        opened(),
        line(1, "Rice 5kg", 48_000, 1_000),
        tender(
            TenderMethod::MobileWallet {
                wallet: Wallet::Bkash,
            },
            30_000,
        ),
        tender(TenderMethod::Cash, 18_000),
        completed(48_000, 0),
    ])
    .expect("valid sequence");

    assert_eq!(sale.tenders().len(), 2);
    // Only the cash portion reaches the drawer.
    assert_eq!(sale.net_cash(), Ok(bdt(18_000)));
}

#[test]
fn a_card_cannot_be_charged_more_than_the_sale() {
    // Only cash can over-tender, because only cash can be handed back. Giving change against a card
    // over-charge takes real money out of the drawer for a payment that never arrived.
    let result = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        tender(TenderMethod::Card, 10_000),
    ]);

    assert_eq!(
        result,
        Err(SaleError::NonCashOvertender {
            tendered: bdt(10_000),
            total: bdt(5_500),
        })
    );
}

#[test]
fn cash_may_overtender_because_change_can_be_given() {
    let sale = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        tender(TenderMethod::Cash, 100_000),
    ])
    .expect("valid sequence");

    assert_eq!(sale.change_due(), Ok(bdt(94_500)));
}

#[test]
fn a_voided_line_leaves_the_total_but_stays_on_the_record() {
    // The whole basis of the fraud wedge: the evidence survives the void.
    let sale = Sale::replay(&[
        opened(),
        line(1, "Rice 5kg", 48_000, 1_000),
        line(2, "Cooking oil 2L", 34_000, 1_000),
        SaleEvent::LineVoided {
            sale_id: id(SALE),
            line_id: id(2),
            reason: VoidReason::CustomerChanged,
            authorized_by: id(0x11A),
        },
        tender(TenderMethod::Cash, 48_000),
        completed(48_000, 0),
    ])
    .expect("valid sequence");

    assert_eq!(sale.settled_total(), Some(bdt(48_000)));
    assert_eq!(sale.lines().len(), 2, "the voided line is retained");
    assert_eq!(sale.active_lines().count(), 1);
    assert_eq!(sale.void_count(), 1);
}

#[test]
fn a_line_cannot_be_voided_twice() {
    let void = SaleEvent::LineVoided {
        sale_id: id(SALE),
        line_id: id(1),
        reason: VoidReason::Mistake,
        authorized_by: id(0x11A),
    };
    let result = Sale::replay(&[opened(), line(1, "Bread", 5_500, 1_000), void.clone(), void]);

    assert_eq!(result, Err(SaleError::AlreadyVoided { line_id: id(1) }));
}

#[test]
fn a_completed_sale_is_immutable() {
    // Adding to a closed sale would let a cashier attach goods to an already-paid ticket.
    let result = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        tender(TenderMethod::Cash, 5_500),
        completed(5_500, 0),
        line(2, "Smuggled", 99_900, 1_000),
    ]);

    assert_eq!(
        result,
        Err(SaleError::NotOpen {
            status: "completed"
        })
    );
}

#[test]
fn an_abandoned_ticket_is_recorded_not_erased() {
    // A cart full of scanned goods walked away from is itself a signal worth showing an owner.
    let sale = Sale::replay(&[
        opened(),
        line(1, "Rice 5kg", 48_000, 1_000),
        SaleEvent::Abandoned {
            sale_id: id(SALE),
            abandoned_by: id(CASHIER),
        },
    ])
    .expect("valid sequence");

    assert_eq!(sale.status(), SaleStatus::Abandoned);
    assert_eq!(sale.lines().len(), 1);
}

#[test]
fn a_weighed_line_prices_exactly() {
    // 1.234 kg at BDT 80.00/kg.
    let sale = Sale::replay(&[
        opened(),
        line(1, "Rice loose", 8_000, 1_234),
        tender(TenderMethod::Cash, 9_872),
        completed(9_872, 0),
    ])
    .expect("valid sequence");

    assert_eq!(sale.settled_total(), Some(bdt(9_872)));
}

#[test]
fn an_order_discount_reduces_the_settled_total() {
    let sale = Sale::replay(&[
        opened(),
        line(1, "Rice 5kg", 48_000, 1_000),
        line(2, "Cooking oil 2L", 34_000, 1_000),
        SaleEvent::OrderDiscounted {
            sale_id: id(SALE),
            discount: Discount::Amount { amount: bdt(2_000) },
            authorized_by: id(0x11A),
        },
        tender(TenderMethod::Cash, 80_000),
        completed(80_000, 0),
    ])
    .expect("valid sequence");

    assert_eq!(sale.settled_total(), Some(bdt(80_000)));
}

#[test]
fn a_sale_with_every_line_voided_cannot_close() {
    let result = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        SaleEvent::LineVoided {
            sale_id: id(SALE),
            line_id: id(1),
            reason: VoidReason::Mistake,
            authorized_by: id(0x11A),
        },
        completed(0, 0),
    ]);

    assert_eq!(result, Err(SaleError::NoActiveLines));
}

#[test]
fn events_for_another_sale_are_refused() {
    // Two tills selling at once must not have their logs cross-contaminate.
    let foreign = SaleEvent::LineAdded {
        sale_id: id(0xBEEF),
        line_id: id(9),
        product_id: id(9),
        name: "Elsewhere".to_owned(),
        unit_price: bdt(100),
        quantity: Quantity::ONE,
        tax_class: VAT_15,
    };
    let result = Sale::replay(&[opened(), foreign]);

    assert_eq!(
        result,
        Err(SaleError::WrongSale {
            expected: id(SALE),
            found: id(0xBEEF),
        })
    );
}

#[test]
fn a_log_that_does_not_begin_with_opened_is_refused() {
    let result = Sale::replay(&[line(1, "Bread", 5_500, 1_000)]);
    assert!(matches!(result, Err(SaleError::NotOpenedFirst { .. })));
}

#[test]
fn a_zero_or_negative_tender_is_refused() {
    // A negative tender would be a way to pull money out of a sale.
    let result = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        tender(TenderMethod::Cash, -1_000),
    ]);
    assert!(matches!(result, Err(SaleError::NonPositiveTender { .. })));
}

#[test]
fn replay_is_deterministic() {
    // Terminal and server both do this; divergence here would mean two versions of a merchant's day.
    let events = vec![
        opened(),
        line(1, "Rice 5kg", 48_000, 1_000),
        line(2, "Bread", 5_500, 3_000),
        SaleEvent::LineVoided {
            sale_id: id(SALE),
            line_id: id(2),
            reason: VoidReason::Damaged,
            authorized_by: id(0x11A),
        },
        tender(TenderMethod::Cash, 50_000),
        completed(48_000, 2_000),
    ];

    assert_eq!(
        Sale::replay(&events).expect("valid"),
        Sale::replay(&events).expect("valid")
    );
}

#[test]
fn the_drawer_opens_only_when_cash_was_involved() {
    let card_only = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        tender(TenderMethod::Card, 5_500),
    ])
    .expect("valid");
    assert!(!card_only.needs_drawer());

    let with_cash = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        tender(TenderMethod::Cash, 5_500),
    ])
    .expect("valid");
    assert!(with_cash.needs_drawer());
}

#[test]
fn a_fully_comped_sale_closes_with_no_tender_at_all() {
    // Surfaced by the replay-determinism generator rather than designed for: a discount can take a
    // basket to exactly zero — a staff meal, a promotional giveaway, a goodwill write-off. Handing
    // over nothing is not a payment, so there is no tender event; a zero balance simply is not
    // outstanding, and the sale closes.
    let sale = Sale::replay(&[
        opened(),
        line(1, "Bread", 5_500, 1_000),
        SaleEvent::OrderDiscounted {
            sale_id: id(SALE),
            discount: Discount::Amount { amount: bdt(5_500) },
            authorized_by: id(0x11A),
        },
        completed(0, 0),
    ])
    .expect("a comped sale is valid");

    assert_eq!(sale.status(), SaleStatus::Completed);
    assert_eq!(sale.settled_total(), Some(bdt(0)));
    assert!(sale.tenders().is_empty());
    assert_eq!(sale.net_cash(), Ok(bdt(0)));
    assert!(
        !sale.needs_drawer(),
        "no cash moved, so the drawer stays shut"
    );
}

// --- Ticket leases -------------------------------------------------------------------------

use sahl_core::Timestamp;
use sahl_core::policy::lease::ClaimVerdict;

const WAITER_A: u128 = 0xA;
const WAITER_B: u128 = 0xB;

fn minute(n: i64) -> Timestamp {
    Timestamp::from_millis(1_753_000_000_000 + n * 60 * 1_000)
}

fn claim(device: u128, at_minute: i64) -> SaleEvent {
    SaleEvent::TicketClaimed {
        sale_id: id(SALE),
        device_id: id(device),
        at: minute(at_minute),
    }
}

#[test]
fn an_unclaimed_ticket_is_writable_by_anyone() {
    // The ordinary retail case: a ticket opens and closes on one till and leases never come up.
    let sale = Sale::replay(&[opened(), line(1, "Bread", 5_500, 1_000)]).expect("valid");

    assert_eq!(sale.lease(), None);
    assert_eq!(sale.may_write(id(WAITER_B), minute(0)), ClaimVerdict::Free);
}

#[test]
fn a_claim_gives_the_ticket_to_one_waiter() {
    let sale = Sale::replay(&[opened(), claim(WAITER_A, 0)]).expect("valid");

    assert_eq!(sale.lease().expect("held").holder, id(WAITER_A));
    assert_eq!(
        sale.may_write(id(WAITER_A), minute(1)),
        ClaimVerdict::AlreadyHeld
    );
    assert_eq!(
        sale.may_write(id(WAITER_B), minute(1)),
        ClaimVerdict::Held {
            holder: id(WAITER_A)
        }
    );
}

#[test]
fn an_idle_ticket_becomes_takeable_but_contested() {
    let sale = Sale::replay(&[opened(), claim(WAITER_A, 0)]).expect("valid");
    let verdict = sale.may_write(id(WAITER_B), minute(11));

    assert!(verdict.permits_claim());
    assert!(
        verdict.is_contested(),
        "the UI must warn before firing a course"
    );
}

#[test]
fn a_contested_claim_replays_to_the_same_holder_either_way() {
    // Both devices and the server see these two claims in whatever order sync delivers them, and
    // must land on the same waiter. Earliest wins.
    let forwards =
        Sale::replay(&[opened(), claim(WAITER_B, 3), claim(WAITER_A, 7)]).expect("valid");
    let backwards =
        Sale::replay(&[opened(), claim(WAITER_A, 7), claim(WAITER_B, 3)]).expect("valid");

    assert_eq!(forwards.lease().expect("held").holder, id(WAITER_B));
    assert_eq!(backwards.lease().expect("held").holder, id(WAITER_B));
}

#[test]
fn replay_accepts_a_contest_rather_than_refusing_it() {
    // A valid log must always replay. Refusing here would mean the server could not ingest a batch
    // describing something that genuinely happened.
    assert!(Sale::replay(&[opened(), claim(WAITER_A, 0), claim(WAITER_B, 0)]).is_ok());
}

#[test]
fn the_holder_can_hand_the_ticket_over() {
    let sale = Sale::replay(&[
        opened(),
        claim(WAITER_A, 0),
        SaleEvent::TicketReleased {
            sale_id: id(SALE),
            device_id: id(WAITER_A),
        },
    ])
    .expect("valid");

    assert_eq!(sale.lease(), None);
    assert_eq!(sale.may_write(id(WAITER_B), minute(1)), ClaimVerdict::Free);
}

#[test]
fn a_release_from_a_waiter_who_lost_the_ticket_is_ignored() {
    // A stale message arriving after they already lost it must not free someone else's ticket.
    let sale = Sale::replay(&[
        opened(),
        claim(WAITER_A, 0),
        SaleEvent::TicketReleased {
            sale_id: id(SALE),
            device_id: id(WAITER_B),
        },
    ])
    .expect("valid");

    assert_eq!(sale.lease().expect("still held").holder, id(WAITER_A));
}
