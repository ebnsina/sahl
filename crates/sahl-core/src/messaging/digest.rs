//! What the shop did, in a sentence somebody reads on a phone.
//!
//! Composed here rather than in whatever sends it, so the wording can be tested without a network
//! and changed without deploying anything that holds provider credentials.
//!
//! Amounts are rendered here too, which is the exception the QR payload also is: a message is
//! bytes going out, and there is no later stage that could format them. Western digits and no
//! grouping — `Intl` is not available to Rust, three providers render three dialects of markup,
//! and a lakh separator in an SMS to somebody who reads Western grouping is a number misread by a
//! factor of ten.

use uuid::Uuid;

use crate::inventory::InventoryBook;
use crate::money::Money;
use crate::quantity::Quantity;
use crate::report::Day;

use super::channel::{Audience, Message, MessageKind};

/// The day, for an owner who is somewhere else.
///
/// Deliberately short. This lands on a phone beside every other notification, and an owner who
/// stops opening it learns nothing — so it carries the four figures that would make somebody ask a
/// question, and nothing that would not.
#[must_use]
pub fn closing_summary(outlet_id: Uuid, shop: &str, day: &Day) -> Message {
    let mut body = format!(
        "{shop}: {} taken over {} sale{}.",
        amount(day.takings),
        day.sales,
        plural(day.sales)
    );

    if !day.discount.is_zero() {
        body.push_str(&format!(" {} discounted.", amount(day.discount)));
    }
    if day.voids > 0 {
        body.push_str(&format!(" {} line{} voided.", day.voids, plural(day.voids)));
    }

    // The busiest hand, but only when there is more than one — "Ruma took all of it" is not a
    // fact about Ruma when Ruma was the only person working.
    if day.by_cashier.len() > 1
        && let Some(top) = day.by_cashier.iter().max_by_key(|row| row.takings.minor())
    {
        body.push_str(&format!(" Busiest: {}.", short(top.staff_id)));
    }

    Message {
        outlet_id,
        kind: MessageKind::ClosingSummary,
        audience: Audience::Owner,
        body,
    }
}

/// What is running out.
///
/// `below` is the level an owner set. Nothing here invents a threshold: "low" for a sack of rice
/// and for a bottle of saffron are different numbers, and a default would be wrong for both.
///
/// Returns `None` when nothing is low — a message saying everything is fine is a message that
/// teaches somebody to ignore the next one.
#[must_use]
pub fn low_stock(
    outlet_id: Uuid,
    shop: &str,
    stock: &InventoryBook,
    below: Quantity,
    name_of: impl Fn(Uuid) -> Option<String>,
) -> Option<Message> {
    let mut running_out: Vec<(String, Quantity)> = stock
        .levels()
        .iter()
        .filter(|level| level.on_hand.milli() <= below.milli())
        .filter_map(|level| name_of(level.batch.product_id).map(|name| (name, level.on_hand)))
        .collect();

    if running_out.is_empty() {
        return None;
    }

    // Emptiest first: the one about to stop a sale is the one worth reading.
    running_out.sort_by_key(|(name, on_hand)| (on_hand.milli(), name.clone()));

    let listed: Vec<String> = running_out
        .iter()
        .take(5)
        .map(|(name, on_hand)| format!("{name} ({})", quantity(*on_hand)))
        .collect();

    let mut body = format!("{shop}: running low — {}", listed.join(", "));
    if running_out.len() > listed.len() {
        body.push_str(&format!(
            " and {} more",
            running_out.len().saturating_sub(listed.len())
        ));
    }
    body.push('.');

    Some(Message {
        outlet_id,
        kind: MessageKind::LowStock,
        audience: Audience::Owner,
        body,
    })
}

/// An amount, in Western digits with no grouping.
fn amount(money: Money) -> String {
    let places = usize::from(money.currency().exponent());
    let per_major = money.currency().minor_per_major().unsigned_abs();
    let magnitude = money.minor().unsigned_abs();
    let sign = if money.minor() < 0 { "-" } else { "" };
    let major = magnitude.div_euclid(per_major);
    let rest = magnitude.rem_euclid(per_major);

    if places == 0 {
        format!("{sign}{} {major}", money.currency().code())
    } else {
        format!("{sign}{} {major}.{rest:0places$}", money.currency().code())
    }
}

/// A quantity, without trailing zeros nobody needs.
fn quantity(value: Quantity) -> String {
    let whole = value.milli().div_euclid(Quantity::MILLI_PER_UNIT);
    let fraction = value.milli().rem_euclid(Quantity::MILLI_PER_UNIT);
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{:03}", fraction)
            .trim_end_matches('0')
            .to_owned()
    }
}

/// Enough of an id to match against a screen. Names live in the directory, which this module
/// deliberately does not take — composing a sentence should not need the staff table.
fn short(id: Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::Channel;
    use crate::money::Currency;
    use crate::report::CashierRow;

    const BDT: Currency = Currency::Bdt;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, BDT)
    }

    fn day(takings: i64, sales: usize) -> Day {
        Day {
            takings: bdt(takings),
            sales,
            ..Day::empty(BDT)
        }
    }

    #[test]
    fn the_summary_leads_with_what_was_taken() {
        let message = closing_summary(id(1), "Karim Store", &day(123_450, 37));

        assert!(
            message
                .body
                .starts_with("Karim Store: BDT 1234.50 taken over 37 sales.")
        );
        assert_eq!(message.kind, MessageKind::ClosingSummary);
        assert_eq!(message.audience, Audience::Owner);
    }

    #[test]
    fn a_single_sale_is_not_pluralised() {
        let message = closing_summary(id(1), "Shop", &day(10_000, 1));
        assert!(message.body.contains("1 sale."), "{}", message.body);
    }

    #[test]
    fn a_quiet_day_still_says_so_rather_than_nothing() {
        // An owner who gets no message cannot tell a quiet day from a broken till.
        let message = closing_summary(id(1), "Shop", &day(0, 0));
        assert!(message.body.contains("BDT 0.00 taken over 0 sales"));
    }

    #[test]
    fn discounts_and_voids_appear_only_when_there_were_some() {
        let quiet = closing_summary(id(1), "Shop", &day(10_000, 2));
        assert!(!quiet.body.contains("discounted"));
        assert!(!quiet.body.contains("voided"));

        let busy = closing_summary(
            id(1),
            "Shop",
            &Day {
                discount: bdt(500),
                voids: 3,
                ..day(10_000, 2)
            },
        );
        assert!(busy.body.contains("BDT 5.00 discounted"));
        assert!(busy.body.contains("3 lines voided"));
    }

    #[test]
    fn the_busiest_hand_is_named_only_when_there_was_more_than_one() {
        // "Ruma took all of it" is not a fact about Ruma when Ruma was the only person working.
        let alone = Day {
            by_cashier: vec![CashierRow {
                staff_id: id(0xCA),
                sales: 2,
                takings: bdt(10_000),
                discount: bdt(0),
                voids: 0,
            }],
            ..day(10_000, 2)
        };
        assert!(
            !closing_summary(id(1), "Shop", &alone)
                .body
                .contains("Busiest")
        );

        let together = Day {
            by_cashier: vec![
                CashierRow {
                    staff_id: id(0xCA),
                    sales: 1,
                    takings: bdt(2_000),
                    discount: bdt(0),
                    voids: 0,
                },
                CashierRow {
                    staff_id: id(0xCB),
                    sales: 3,
                    takings: bdt(8_000),
                    discount: bdt(0),
                    voids: 0,
                },
            ],
            ..day(10_000, 4)
        };
        let message = closing_summary(id(1), "Shop", &together);
        assert!(
            message.body.contains("Busiest: 00000000"),
            "{}",
            message.body
        );
    }

    #[test]
    fn an_ordinary_summary_fits_in_one_sms_segment() {
        // Every segment after the first is billed again, and a daily message is billed every day.
        let message = closing_summary(id(1), "Karim Store", &day(123_450, 37));
        assert!(
            message.fits(Channel::Sms),
            "{} chars: {}",
            message.body.chars().count(),
            message.body
        );
    }

    #[test]
    fn amounts_are_western_digits_with_no_grouping() {
        // A lakh separator in an SMS to somebody reading Western grouping is a number misread by
        // a factor of ten.
        assert_eq!(amount(bdt(10_000_000)), "BDT 100000.00");
        assert_eq!(amount(bdt(5)), "BDT 0.05");
        assert_eq!(amount(bdt(-1_50)), "-BDT 1.50");
    }

    /// A book holding `on_hand` of one product.
    fn stocked(product: u128, on_hand: i64) -> crate::inventory::InventoryEvent {
        crate::inventory::InventoryEvent::BatchReceived {
            batch_id: id(product + 1_000),
            product_id: id(product),
            lot: None,
            expires_at: None,
            quantity: Quantity::from_milli(on_hand),
            unit_cost: bdt(100),
            supplier: None,
            at: crate::time::Timestamp::from_millis(1_753_000_000_000),
            received_by: id(0x0E),
        }
    }

    fn book(events: &[crate::inventory::InventoryEvent]) -> InventoryBook {
        InventoryBook::replay(events).expect("valid")
    }

    fn names(product: Uuid) -> Option<String> {
        match product.as_u128() {
            1 => Some("Rice".to_owned()),
            2 => Some("Sugar".to_owned()),
            3 => Some("Salt".to_owned()),
            _ => None,
        }
    }

    #[test]
    fn a_shop_with_nothing_low_is_told_nothing() {
        // A message saying everything is fine teaches somebody to ignore the next one.
        let stock = book(&[stocked(1, 40_000)]);
        assert_eq!(
            low_stock(id(9), "Shop", &stock, Quantity::from_milli(5_000), names),
            None
        );
    }

    #[test]
    fn what_is_running_out_is_named_with_what_is_left() {
        let stock = book(&[stocked(1, 1_000), stocked(2, 40_000)]);
        let message = low_stock(id(9), "Shop", &stock, Quantity::from_milli(5_000), names)
            .expect("something is low");

        assert!(message.body.contains("Rice (1)"), "{}", message.body);
        assert!(!message.body.contains("Sugar"), "sugar is fine");
        assert_eq!(message.kind, MessageKind::LowStock);
    }

    #[test]
    fn the_emptiest_comes_first() {
        // The one about to stop a sale is the one worth reading.
        let stock = book(&[stocked(1, 4_000), stocked(2, 500), stocked(3, 2_000)]);
        let message =
            low_stock(id(9), "Shop", &stock, Quantity::from_milli(5_000), names).expect("low");

        let sugar = message.body.find("Sugar").expect("listed");
        let rice = message.body.find("Rice").expect("listed");
        assert!(sugar < rice, "{}", message.body);
    }

    #[test]
    fn a_long_list_is_summarised_rather_than_sent_whole() {
        // Twenty product names is not a notification anybody reads on a phone.
        let events: Vec<_> = (1..=8).map(|n| stocked(n, 100)).collect();
        let stock = book(&events);
        let message = low_stock(
            id(9),
            "Shop",
            &stock,
            Quantity::from_milli(5_000),
            |product| Some(format!("Item {}", product.as_u128())),
        )
        .expect("low");

        assert!(message.body.contains("and 3 more"), "{}", message.body);
    }

    #[test]
    fn a_product_this_build_cannot_name_is_left_out_rather_than_called_unknown() {
        // "Unknown (2)" tells an owner nothing they can act on, and the catalogue entry is
        // probably one sync away.
        let stock = book(&[stocked(99, 100)]);
        assert_eq!(
            low_stock(id(9), "Shop", &stock, Quantity::from_milli(5_000), names),
            None
        );
    }

    #[test]
    fn a_quantity_drops_the_zeros_nobody_needs() {
        assert_eq!(quantity(Quantity::from_milli(3_000)), "3");
        assert_eq!(quantity(Quantity::from_milli(1_250)), "1.25");
    }
}
