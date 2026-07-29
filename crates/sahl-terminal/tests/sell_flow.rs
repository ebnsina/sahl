//! The sell flow, end to end through the till.
//!
//! Drives a real store and a real chain the way the sell screen does, and asserts the numbers the
//! screen would render. That is stronger evidence than a screenshot: a screenshot shows that
//! something appeared, these show that the amounts are right.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use sahl_core::Timestamp;
use sahl_core::money::{Currency, Money, Rounding};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{SaleEvent, TenderMethod, VoidReason};
use sahl_core::tax::{PricingMode, TaxClass};
use sahl_terminal_lib::commands::SaleView;
use sahl_terminal_lib::store::EventStore;
use sahl_terminal_lib::{DeviceIdentity, Terminal};
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn bdt(minor: i64) -> Money {
    Money::from_minor(minor, BDT)
}

struct Till {
    terminal: Terminal,
    sale_id: Uuid,
    clock: i64,
    counter: u128,
}

impl Till {
    fn new() -> Self {
        let terminal = Terminal::load(
            EventStore::open_in_memory().expect("opens"),
            DeviceIdentity {
                tenant_id: id(1),
                outlet_id: id(2),
                device_id: id(3),
            },
        )
        .expect("loads");

        Self {
            terminal,
            sale_id: id(0x5A1E),
            clock: 1_753_000_000_000,
            counter: 0,
        }
    }

    fn record(&mut self, event: SaleEvent) -> SaleView {
        self.counter += 1;
        self.clock += 1;
        self.terminal
            .record(
                &event,
                id(0x1000 + self.counter),
                Timestamp::from_millis(self.clock),
            )
            .expect("records");
        self.view()
    }

    fn view(&self) -> SaleView {
        SaleView::of(self.terminal.sale(self.sale_id).expect("sale")).expect("view")
    }

    fn open(&mut self) -> SaleView {
        let sale_id = self.sale_id;
        self.record(SaleEvent::Opened {
            sale_id,
            opened_by: id(0xCA51),
            currency: BDT,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        })
    }

    fn add(&mut self, line: u128, name: &str, minor: i64, milli: i64, bp: i32) -> SaleView {
        let sale_id = self.sale_id;
        self.record(SaleEvent::LineAdded {
            sale_id,
            line_id: id(line),
            product_id: id(line + 500),
            name: name.to_owned(),
            unit_price: bdt(minor),
            quantity: Quantity::from_milli(milli),
            tax_class: if bp == 0 {
                TaxClass::Exempt
            } else {
                TaxClass::standard(bp)
            },
        })
    }

    fn cash(&mut self, minor: i64) -> SaleView {
        let sale_id = self.sale_id;
        self.counter += 1;
        let tender_id = id(0x9000 + self.counter);
        self.record(SaleEvent::TenderRecorded {
            sale_id,
            tender_id,
            method: TenderMethod::Cash,
            amount: bdt(minor),
            reference: None,
        })
    }
}

#[test]
fn a_mixed_rate_basket_shows_the_numbers_a_cashier_expects() {
    // Rice at 15%, bread at 7.5%, milk exempt — the everyday case in the launch market.
    let mut till = Till::new();
    till.open();
    till.add(1, "Basmati rice 5kg", 48_000, 1_000, 1500);
    till.add(2, "Bread", 5_500, 2_000, 750);
    let view = till.add(3, "Fresh milk 1L", 9_000, 1_000, 0);

    // Tax-inclusive: the customer pays exactly the shelf prices summed.
    assert_eq!(view.total_minor, 48_000 + 11_000 + 9_000);

    // Three distinct tax treatments, in conventional invoice order: 7.5%, 15%, then exempt.
    let classes: Vec<_> = view
        .tax_groups
        .iter()
        .map(|g| (g.class, g.basis_points))
        .collect();
    assert_eq!(
        classes,
        vec![("standard", 750), ("standard", 1500), ("exempt", 0)]
    );

    // The summary accounts for every taka: bases plus taxes reconstruct the total.
    let summed: i64 = view
        .tax_groups
        .iter()
        .map(|g| g.taxable_base_minor + g.tax_minor)
        .sum();
    assert_eq!(summed, view.total_minor);
    assert_eq!(view.net_minor + view.tax_minor, view.total_minor);
}

#[test]
fn a_weighed_line_prices_exactly() {
    let mut till = Till::new();
    till.open();
    // 1.234 kg at BDT 80.00/kg.
    let view = till.add(1, "Rice loose", 8_000, 1_234, 1500);

    assert_eq!(view.total_minor, 9_872);
    assert_eq!(view.lines[0].quantity_milli, 1_234);
}

#[test]
fn change_is_computed_by_the_till_not_the_screen() {
    let mut till = Till::new();
    till.open();
    till.add(1, "Bread", 5_500, 1_000, 750);
    let view = till.cash(10_000);

    assert_eq!(view.balance_due_minor, -4_500, "over-tendered");
    assert_eq!(view.change_due_minor, 4_500);
    assert!(view.needs_drawer, "cash moved, so the drawer opens");
}

#[test]
fn a_voided_line_stays_visible_and_struck_through() {
    // The screen must show the void, not hide it — that is the whole point of keeping it.
    let mut till = Till::new();
    till.open();
    till.add(1, "Basmati rice 5kg", 48_000, 1_000, 1500);
    till.add(2, "Cooking oil 2L", 34_000, 1_000, 1500);

    let sale_id = till.sale_id;
    let view = till.record(SaleEvent::LineVoided {
        sale_id,
        line_id: id(2),
        reason: VoidReason::Mistake,
        authorized_by: id(0x11A),
    });

    assert_eq!(view.lines.len(), 2, "the voided line is still rendered");
    assert!(view.lines[1].voided);
    assert_eq!(view.lines[1].total_minor, 0, "and contributes nothing");
    assert_eq!(view.total_minor, 48_000);
    assert_eq!(view.void_count, 1);
}

#[test]
fn voiding_the_only_line_renders_an_empty_cart_rather_than_an_error() {
    // A cashier who voids their last line should see an empty cart, not a crash.
    let mut till = Till::new();
    till.open();
    till.add(1, "Bread", 5_500, 1_000, 750);

    let sale_id = till.sale_id;
    let view = till.record(SaleEvent::LineVoided {
        sale_id,
        line_id: id(1),
        reason: VoidReason::CustomerChanged,
        authorized_by: id(0x11A),
    });

    assert_eq!(view.total_minor, 0);
    assert_eq!(view.lines.len(), 1);
    assert!(view.tax_groups.is_empty());
}

#[test]
fn a_completed_sale_reports_what_was_paid() {
    let mut till = Till::new();
    till.open();
    till.add(1, "Basmati rice 5kg", 48_000, 1_000, 1500);
    till.cash(50_000);

    let sale_id = till.sale_id;
    let view = till.record(SaleEvent::Completed {
        sale_id,
        total: bdt(48_000),
        change_given: bdt(2_000),
    });

    assert_eq!(view.status, "completed");
    assert_eq!(view.total_minor, 48_000);
    assert_eq!(view.change_due_minor, 2_000);
}

#[test]
fn split_payment_leaves_the_right_balance_at_each_step() {
    let mut till = Till::new();
    till.open();
    till.add(1, "Basmati rice 5kg", 48_000, 1_000, 1500);

    let after_first = till.cash(20_000);
    assert_eq!(after_first.balance_due_minor, 28_000);
    assert_eq!(after_first.change_due_minor, 0);

    let after_second = till.cash(28_000);
    assert_eq!(after_second.balance_due_minor, 0);
    assert_eq!(after_second.change_due_minor, 0);
    assert_eq!(after_second.tenders.len(), 2);
}

#[test]
fn every_amount_the_screen_shows_is_an_exact_integer() {
    // The view carries no formatted strings and no floats. A float here would undo the whole money
    // design at the very last step, so this is asserted on the serialized payload itself.
    let mut till = Till::new();
    till.open();
    till.add(1, "Bread", 5_500, 1_333, 750);
    let view = till.cash(10_000);

    let json = serde_json::to_value(&view).expect("serialises");
    let mut checked = 0;
    fn walk(value: &serde_json::Value, checked: &mut usize) {
        match value {
            serde_json::Value::Number(number) => {
                assert!(
                    number.is_i64() || number.is_u64(),
                    "found a non-integer amount: {number}"
                );
                *checked += 1;
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, checked)),
            serde_json::Value::Object(entries) => {
                entries.values().for_each(|entry| walk(entry, checked));
            }
            _ => {}
        }
    }
    walk(&json, &mut checked);
    assert!(checked > 10, "expected many numeric fields, saw {checked}");
}
