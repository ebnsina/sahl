use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::{EventError, canonical_bytes};
use crate::money::{Currency, Money};
use crate::sale::{Sale, SaleError, SaleEvent, SaleStatus};

/// Every sale a device knows about, rebuilt from its events.
///
/// Keyed by a `BTreeMap` rather than a `HashMap` deliberately: hash iteration order varies between
/// processes, so any report, sync payload or fingerprint derived by iterating one would differ
/// between two machines holding identical data. Sale ids are UUID v7, so btree order is also
/// creation order — which happens to be the order a shift report wants anyway.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleBook {
    sales: BTreeMap<Uuid, Sale>,
}

impl SaleBook {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild from a full event stream, which may interleave several sales.
    ///
    /// Interleaving is the normal case, not an edge case: a café has several tickets open at once,
    /// and even a retail till can have a parked sale while it serves someone else.
    ///
    /// # Errors
    /// [`SaleError`] if the stream is inconsistent.
    pub fn replay(events: &[SaleEvent]) -> Result<Self, SaleError> {
        let mut book = Self::new();
        for event in events {
            book.apply(event)?;
        }
        Ok(book)
    }

    /// Apply one event, opening a new sale if this is its first.
    ///
    /// # Errors
    /// [`SaleError`] if the event is not valid for the sale's current state.
    pub fn apply(&mut self, event: &SaleEvent) -> Result<(), SaleError> {
        let sale_id = event.sale_id();

        match self.sales.get_mut(&sale_id) {
            Some(sale) => sale.apply(event),
            None => {
                // Only an `Opened` event may create a sale; anything else arriving first means the
                // stream is truncated or out of order, which `Sale::replay` reports precisely.
                let sale = Sale::replay(std::slice::from_ref(event))?;
                self.sales.insert(sale_id, sale);
                Ok(())
            }
        }
    }

    #[must_use]
    pub fn get(&self, sale_id: Uuid) -> Option<&Sale> {
        self.sales.get(&sale_id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.sales.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sales.is_empty()
    }

    /// Every sale, in creation order.
    pub fn iter(&self) -> impl Iterator<Item = &Sale> {
        self.sales.values()
    }

    /// Tickets still open — what a café's floor view shows, and what a retail till must not lose
    /// track of when it restarts mid-shift.
    pub fn open(&self) -> impl Iterator<Item = &Sale> {
        self.iter().filter(|sale| sale.status() == SaleStatus::Open)
    }

    pub fn completed(&self) -> impl Iterator<Item = &Sale> {
        self.iter()
            .filter(|sale| sale.status() == SaleStatus::Completed)
    }

    /// Total takings across completed sales.
    ///
    /// Sums the settled totals recorded at completion rather than recalculating, so a shift report
    /// reports what customers were actually charged.
    ///
    /// # Errors
    /// [`SaleError::Money`] on currency mismatch or overflow.
    pub fn takings(&self, currency: Currency) -> Result<Money, SaleError> {
        Ok(Money::try_sum(
            self.completed().filter_map(Sale::settled_total),
            currency,
        )?)
    }

    /// Cash that should be in the drawer, across completed sales.
    ///
    /// # Errors
    /// [`SaleError`] on currency mismatch or overflow.
    pub fn expected_cash(&self, currency: Currency) -> Result<Money, SaleError> {
        self.completed()
            .try_fold(Money::zero(currency), |total, sale| {
                Ok(total.checked_add(sale.net_cash()?)?)
            })
    }

    /// How many lines were voided across every sale — the raw input to the void-rate signal an
    /// owner sees.
    #[must_use]
    pub fn void_count(&self) -> usize {
        self.iter().map(Sale::void_count).sum()
    }

    /// A stable digest of the whole projection.
    ///
    /// This is what proves replay determinism: two machines that have seen the same events must
    /// produce the same bytes here. Reuses the event log's canonical serialization, so it inherits
    /// the same guarantees — sorted keys, no whitespace, and floats refused outright.
    ///
    /// # Errors
    /// [`EventError::NotCanonical`] if the projection cannot be canonically serialized.
    pub fn fingerprint(&self) -> Result<Vec<u8>, EventError> {
        canonical_bytes(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Rounding;
    use crate::quantity::Quantity;
    use crate::sale::TenderMethod;
    use crate::tax::{PricingMode, TaxClass};

    const BDT: Currency = Currency::Bdt;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, BDT)
    }

    fn opened(sale: u128) -> SaleEvent {
        SaleEvent::Opened {
            sale_id: id(sale),
            opened_by: id(0xCA51),
            currency: BDT,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        }
    }

    fn line(sale: u128, line_id: u128, minor: i64) -> SaleEvent {
        SaleEvent::LineAdded {
            sale_id: id(sale),
            line_id: id(line_id),
            product_id: id(line_id + 500),
            name: format!("Item {line_id}"),
            unit_price: bdt(minor),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            modifiers: Vec::new(),
        }
    }

    fn paid(sale: u128, minor: i64) -> Vec<SaleEvent> {
        vec![
            SaleEvent::TenderRecorded {
                sale_id: id(sale),
                tender_id: id(sale + 900),
                method: TenderMethod::Cash,
                amount: bdt(minor),
                reference: None,
            },
            SaleEvent::Completed {
                sale_id: id(sale),
                total: bdt(minor),
                change_given: bdt(0),
                at: crate::Timestamp::from_millis(1_753_000_000_000),
            },
        ]
    }

    /// Two tickets open at once, closing out of order — the café case, and a parked retail sale.
    fn interleaved() -> Vec<SaleEvent> {
        let mut events = vec![
            opened(1),
            line(1, 11, 10_000),
            opened(2),
            line(2, 21, 25_000),
            line(1, 12, 5_000),
        ];
        events.extend(paid(2, 25_000));
        events.extend(paid(1, 15_000));
        events
    }

    #[test]
    fn interleaved_tickets_are_kept_apart() {
        let book = SaleBook::replay(&interleaved()).expect("valid stream");

        assert_eq!(book.len(), 2);
        assert_eq!(
            book.get(id(1)).expect("sale 1").settled_total(),
            Some(bdt(15_000))
        );
        assert_eq!(
            book.get(id(2)).expect("sale 2").settled_total(),
            Some(bdt(25_000))
        );
    }

    #[test]
    fn takings_sum_what_customers_were_actually_charged() {
        let book = SaleBook::replay(&interleaved()).expect("valid stream");
        assert_eq!(book.takings(BDT), Ok(bdt(40_000)));
        assert_eq!(book.expected_cash(BDT), Ok(bdt(40_000)));
    }

    #[test]
    fn open_tickets_are_distinguishable_from_closed_ones() {
        // A till restarting mid-shift has to know which tickets are still live.
        let mut events = vec![
            opened(1),
            line(1, 11, 10_000),
            opened(2),
            line(2, 21, 25_000),
        ];
        events.extend(paid(2, 25_000));
        let book = SaleBook::replay(&events).expect("valid stream");

        assert_eq!(book.open().count(), 1);
        assert_eq!(book.completed().count(), 1);
        assert_eq!(book.open().next().expect("one open").id(), id(1));
    }

    #[test]
    fn replaying_twice_yields_an_identical_fingerprint() {
        let events = interleaved();
        let first = SaleBook::replay(&events).expect("valid stream");
        let second = SaleBook::replay(&events).expect("valid stream");

        assert_eq!(
            first.fingerprint().expect("canonical"),
            second.fingerprint().expect("canonical")
        );
    }

    #[test]
    fn applying_incrementally_matches_a_single_replay() {
        // The terminal applies events one at a time as they happen; the server replays a batch.
        // Both must land in exactly the same place.
        let events = interleaved();
        let batch = SaleBook::replay(&events).expect("valid stream");

        let mut incremental = SaleBook::new();
        for event in &events {
            incremental.apply(event).expect("valid event");
        }

        assert_eq!(
            batch.fingerprint().expect("canonical"),
            incremental.fingerprint().expect("canonical")
        );
    }

    #[test]
    fn an_event_arriving_before_its_sale_was_opened_is_refused() {
        // Means the stream is truncated or out of order — never something to paper over.
        let result = SaleBook::replay(&[line(1, 11, 10_000)]);
        assert!(matches!(result, Err(SaleError::NotOpenedFirst { .. })));
    }

    #[test]
    fn an_empty_book_is_empty() {
        let book = SaleBook::new();
        assert!(book.is_empty());
        assert_eq!(book.takings(BDT), Ok(bdt(0)));
        assert_eq!(book.void_count(), 0);
    }
}
