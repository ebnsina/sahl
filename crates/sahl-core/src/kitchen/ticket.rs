//! What goes to a station, and what has already gone.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::sale::{Sale, SaleLine};
use crate::time::Timestamp;

use super::station::Station;

/// One line as a cook reads it.
///
/// No prices. A kitchen ticket is an instruction, not a bill, and a number beside an item is a
/// number somebody will eventually mistake for a quantity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketLine {
    pub line_id: Uuid,
    pub name: String,
    /// Thousandths, like everywhere else. Printed whole where it is whole.
    pub quantity_milli: i64,
    /// The options, which on a kitchen ticket are the most important part of the line — "no nuts"
    /// matters more than the dish name.
    pub modifiers: Vec<String>,
}

/// Why a station is being sent something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketKind {
    /// Make these.
    Order,
    /// Stop making these — they were voided after the station already had them.
    ///
    /// A separate kind because it is the opposite instruction and must never be mistaken for an
    /// order. A cancellation that reads like an order gets the dish made twice.
    Cancellation,
}

/// One station's instruction for one ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KitchenTicket {
    pub sale_id: Uuid,
    pub station: Station,
    pub kind: TicketKind,
    /// The table, for a café. A ticket with no table is a counter order.
    pub table_label: Option<String>,
    pub covers: Option<u32>,
    /// Which round this is. A cook reading "2" knows the first round is already out.
    pub round: u32,
    pub at: Timestamp,
    pub lines: Vec<TicketLine>,
}

impl KitchenTicket {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Work out what each station has not yet been told about.
///
/// `already_fired` is every line id previously sent, which the sale records — without it a second
/// press of "send" reprints the whole order and the kitchen makes it twice. That is the failure
/// this whole module is arranged around.
///
/// `station_of` resolves a line's station from the catalogue. A line whose product this device has
/// never seen goes to the kitchen: a sibling's catalogue entry can arrive after its first sale, and
/// a ticket printed at the wrong station is recoverable where a ticket never printed is not.
pub fn pending<F>(
    sale: &Sale,
    already_fired: &[Uuid],
    round: u32,
    at: Timestamp,
    table_label: Option<String>,
    station_of: F,
) -> Vec<KitchenTicket>
where
    F: Fn(Uuid) -> Option<Station>,
{
    let mut by_station: std::collections::BTreeMap<Station, Vec<TicketLine>> =
        std::collections::BTreeMap::new();

    for line in sale.active_lines() {
        if already_fired.contains(&line.id) {
            continue;
        }
        let station = station_of(line.product_id).unwrap_or(Station::Kitchen);
        by_station.entry(station).or_default().push(to_line(line));
    }

    by_station
        .into_iter()
        .map(|(station, lines)| KitchenTicket {
            sale_id: sale.id(),
            station,
            kind: TicketKind::Order,
            table_label: table_label.clone(),
            covers: sale.seating().map(|seating| seating.covers),
            round,
            at,
            lines,
        })
        .collect()
}

/// Tickets telling stations to stop making something.
///
/// Only for lines a station was already told about. Voiding a line before it was ever sent needs no
/// ticket — nobody started it — and printing one would have a cook looking for an order they never
/// received.
pub fn cancellations<F>(
    sale: &Sale,
    already_fired: &[Uuid],
    round: u32,
    at: Timestamp,
    table_label: Option<String>,
    station_of: F,
) -> Vec<KitchenTicket>
where
    F: Fn(Uuid) -> Option<Station>,
{
    let mut by_station: std::collections::BTreeMap<Station, Vec<TicketLine>> =
        std::collections::BTreeMap::new();

    for line in sale.lines() {
        if line.is_active() || !already_fired.contains(&line.id) {
            continue;
        }
        let station = station_of(line.product_id).unwrap_or(Station::Kitchen);
        by_station.entry(station).or_default().push(to_line(line));
    }

    by_station
        .into_iter()
        .map(|(station, lines)| KitchenTicket {
            sale_id: sale.id(),
            station,
            kind: TicketKind::Cancellation,
            table_label: table_label.clone(),
            covers: sale.seating().map(|seating| seating.covers),
            round,
            at,
            lines,
        })
        .collect()
}

fn to_line(line: &SaleLine) -> TicketLine {
    TicketLine {
        line_id: line.id,
        name: line.name.clone(),
        quantity_milli: line.quantity.milli(),
        modifiers: line
            .modifiers
            .iter()
            .map(|modifier| modifier.name.clone())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::{Currency, Money, Rounding};
    use crate::quantity::Quantity;
    use crate::sale::{Modifier, SaleEvent, VoidReason};
    use crate::tax::{PricingMode, TaxClass};

    const BDT: Currency = Currency::Bdt;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(n: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + n)
    }

    fn opened() -> SaleEvent {
        SaleEvent::Opened {
            sale_id: id(1),
            opened_by: id(0xCA),
            currency: BDT,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        }
    }

    fn added(line: u128, product: u128, name: &str, options: &[&str]) -> SaleEvent {
        SaleEvent::LineAdded {
            sale_id: id(1),
            line_id: id(line),
            product_id: id(product),
            name: name.to_owned(),
            unit_price: Money::from_minor(30_000, BDT),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            modifiers: options
                .iter()
                .map(|option| Modifier {
                    option_id: id(900),
                    name: (*option).to_owned(),
                    price_delta: Money::from_minor(0, BDT),
                })
                .collect(),
        }
    }

    /// Products 10 and 11 are food; 20 is a drink.
    fn station_of(product: Uuid) -> Option<Station> {
        match product {
            p if p == id(20) => Some(Station::Bar),
            _ => Some(Station::Kitchen),
        }
    }

    fn order() -> Sale {
        Sale::replay(&[
            opened(),
            added(1, 10, "Chicken curry", &["No nuts"]),
            added(2, 11, "Naan", &[]),
            added(3, 20, "Lime soda", &["No ice"]),
        ])
        .expect("valid")
    }

    #[test]
    fn lines_route_to_their_own_station() {
        let tickets = pending(&order(), &[], 1, at(0), Some("4".to_owned()), station_of);

        assert_eq!(tickets.len(), 2, "one for the kitchen, one for the bar");
        let kitchen = tickets
            .iter()
            .find(|ticket| ticket.station == Station::Kitchen)
            .expect("kitchen");
        assert_eq!(kitchen.lines.len(), 2);
        let bar = tickets
            .iter()
            .find(|ticket| ticket.station == Station::Bar)
            .expect("bar");
        assert_eq!(bar.lines.len(), 1);
        assert_eq!(bar.lines[0].name, "Lime soda");
    }

    #[test]
    fn options_travel_with_the_line() {
        // On a kitchen ticket the options matter more than the dish name — "no nuts" is the part
        // that hurts somebody if it is dropped.
        let tickets = pending(&order(), &[], 1, at(0), None, station_of);
        let kitchen = &tickets[0];
        let curry = kitchen
            .lines
            .iter()
            .find(|line| line.name == "Chicken curry")
            .expect("present");

        assert_eq!(curry.modifiers, vec!["No nuts".to_owned()]);
    }

    #[test]
    fn a_line_already_sent_is_not_sent_again() {
        // The failure this module is arranged around: a second press of "send" that reprints the
        // whole order gets the food made twice.
        let tickets = pending(&order(), &[id(1), id(2)], 2, at(0), None, station_of);

        assert_eq!(tickets.len(), 1, "only the bar has anything new");
        assert_eq!(tickets[0].station, Station::Bar);
    }

    #[test]
    fn nothing_new_produces_no_tickets_at_all() {
        // Not an empty ticket. A cook handed a blank slip has to work out whether it means
        // anything, and the answer is always no.
        let tickets = pending(&order(), &[id(1), id(2), id(3)], 2, at(0), None, station_of);
        assert!(tickets.is_empty());
    }

    #[test]
    fn a_second_round_is_numbered_so_a_cook_knows_the_first_is_out() {
        let tickets = pending(&order(), &[id(1)], 2, at(0), None, station_of);
        assert!(tickets.iter().all(|ticket| ticket.round == 2));
    }

    #[test]
    fn voiding_a_line_the_station_already_has_sends_a_cancellation() {
        let mut sale = order();
        sale.apply(&SaleEvent::LineVoided {
            sale_id: id(1),
            line_id: id(1),
            reason: VoidReason::CustomerChanged,
            authorized_by: id(0x11A),
        })
        .expect("voids");

        let cancels = cancellations(&sale, &[id(1), id(2), id(3)], 2, at(0), None, station_of);

        assert_eq!(cancels.len(), 1);
        assert_eq!(cancels[0].kind, TicketKind::Cancellation);
        assert_eq!(cancels[0].station, Station::Kitchen);
        assert_eq!(cancels[0].lines[0].name, "Chicken curry");
    }

    #[test]
    fn voiding_a_line_nobody_started_sends_nothing() {
        // Printing one would have a cook looking for an order they never received.
        let mut sale = order();
        sale.apply(&SaleEvent::LineVoided {
            sale_id: id(1),
            line_id: id(1),
            reason: VoidReason::Mistake,
            authorized_by: id(0x11A),
        })
        .expect("voids");

        let cancels = cancellations(&sale, &[], 2, at(0), None, station_of);
        assert!(cancels.is_empty());
    }

    #[test]
    fn a_voided_line_is_never_on_an_order_ticket() {
        let mut sale = order();
        sale.apply(&SaleEvent::LineVoided {
            sale_id: id(1),
            line_id: id(1),
            reason: VoidReason::Mistake,
            authorized_by: id(0x11A),
        })
        .expect("voids");

        let tickets = pending(&sale, &[], 1, at(0), None, station_of);
        let kitchen = tickets
            .iter()
            .find(|ticket| ticket.station == Station::Kitchen)
            .expect("kitchen");

        assert_eq!(kitchen.lines.len(), 1, "only the naan");
    }

    #[test]
    fn a_product_this_device_has_never_seen_still_reaches_a_station() {
        // A sibling's catalogue entry can arrive after its first sale. A ticket at the wrong
        // station is recoverable; a ticket that never printed is a table waiting on nothing.
        let tickets = pending(&order(), &[], 1, at(0), None, |_| None);
        assert_eq!(tickets.len(), 1);
        assert_eq!(tickets[0].station, Station::Kitchen);
        assert_eq!(tickets[0].lines.len(), 3);
    }

    #[test]
    fn the_table_and_covers_travel_with_the_ticket() {
        let mut sale = order();
        sale.apply(&SaleEvent::Seated {
            sale_id: id(1),
            table_id: id(0x7AB),
            covers: 4,
            at: at(1),
            seated_by: id(0xCA),
        })
        .expect("seats");

        let tickets = pending(&sale, &[], 1, at(2), Some("12".to_owned()), station_of);
        assert_eq!(tickets[0].table_label.as_deref(), Some("12"));
        assert_eq!(tickets[0].covers, Some(4));
    }

    #[test]
    fn stations_come_back_in_a_stable_order() {
        // Two devices firing the same order must produce the same tickets in the same order, or a
        // reprint looks like a different order.
        let first = pending(&order(), &[], 1, at(0), None, station_of);
        let second = pending(&order(), &[], 1, at(0), None, station_of);
        assert_eq!(first, second);
    }
}
