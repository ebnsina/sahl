//! The till's in-process state: the log, the chain, and the projection over it.
//!
//! Everything the UI can do goes through [`Terminal::record`], which does four things in a fixed
//! order and refuses to do any of them partially:
//!
//! 1. Validate the event against the current projection.
//! 2. Seal it into the hash chain.
//! 3. Write it to disk.
//! 4. Apply it to the in-memory projection.
//!
//! The order matters. Validating first means a rejected action never reaches the log; writing to
//! disk before updating memory means the in-memory state can never claim something the disk does
//! not have. A till that shows a sale it did not persist is worse than one that refuses the sale.

use sahl_core::event::{EventChain, EventEnvelope, EventHeader};
use sahl_core::projection::SaleBook;
use sahl_core::sale::{Sale, SaleError, SaleEvent};
use sahl_core::{Money, Timestamp};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::store::{EventStore, StoreError};

/// Who this device is. Fixed at enrollment and never changes for the life of the install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub tenant_id: Uuid,
    pub outlet_id: Uuid,
    pub device_id: Uuid,
}

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("{0}")]
    Sale(#[from] SaleError),

    #[error("{0}")]
    Store(#[from] StoreError),

    #[error("event log error: {0}")]
    Event(#[from] sahl_core::event::EventError),

    #[error("no sale {sale_id}")]
    UnknownSale { sale_id: Uuid },

    /// The stored log did not verify on load. Not recoverable at the till: the device must sync
    /// what it can and be re-enrolled, because nothing it reports afterwards can be trusted.
    #[error("the local event log failed verification: {reason}")]
    CorruptLog { reason: String },
}

#[derive(Debug)]
pub struct Terminal {
    store: EventStore,
    chain: EventChain,
    book: SaleBook,
    identity: DeviceIdentity,
}

impl Terminal {
    /// Load a terminal from its store, rebuilding the projection and verifying the chain.
    ///
    /// Verification on load is deliberate. It costs a pass over the log at startup and it is the
    /// only moment a tampered local database is cheap to catch — before the device starts writing
    /// new events on top of a chain that no longer holds.
    ///
    /// # Errors
    /// [`TerminalError::CorruptLog`] if the stored chain does not verify.
    pub fn load(store: EventStore, identity: DeviceIdentity) -> Result<Self, TerminalError> {
        let stored = store.load_all()?;

        sahl_core::event::verify_chain_from_genesis(&stored).map_err(|error| {
            TerminalError::CorruptLog {
                reason: error.to_string(),
            }
        })?;

        let mut book = SaleBook::new();
        for envelope in &stored {
            // Only sale events project into the book; other kinds (shifts, stock) will have their
            // own projections and are skipped rather than treated as an error.
            if let Ok(event) = envelope.payload_as::<SaleEvent>() {
                book.apply(&event)?;
            }
        }

        let chain = EventChain::resume(identity.device_id, store.tip()?);
        Ok(Self {
            store,
            chain,
            book,
            identity,
        })
    }

    /// Validate, seal, persist, and project one event.
    ///
    /// # Errors
    /// [`TerminalError`] if the event is invalid for the current state or cannot be persisted.
    pub fn record(
        &mut self,
        event: &SaleEvent,
        event_id: Uuid,
        occurred_at: Timestamp,
    ) -> Result<EventEnvelope, TerminalError> {
        // 1. Validate against a throwaway copy first. If this fails, nothing has been written and
        //    the till is exactly as it was — which is what lets the UI surface an error without
        //    having to undo anything.
        let mut candidate = self.book.clone();
        candidate.apply(event)?;

        // 2. Seal into the chain.
        let header = EventHeader {
            event_id,
            tenant_id: self.identity.tenant_id,
            outlet_id: self.identity.outlet_id,
            device_id: self.identity.device_id,
            occurred_at,
        };
        let envelope = self.chain.append(header, event)?;

        // 3. Persist before believing it.
        self.store.append(&envelope)?;

        // 4. Only now adopt the new state.
        self.book = candidate;
        Ok(envelope)
    }

    /// Split into store and projection, so the sync loop can drive the store directly.
    ///
    /// Sync needs `&mut EventStore` while the till holds it; handing the parts over is simpler and
    /// more honest than lending a mutable interior reference through the aggregate.
    #[must_use]
    pub fn into_parts(self) -> (EventStore, SaleBook) {
        (self.store, self.book)
    }

    #[must_use]
    pub const fn identity(&self) -> DeviceIdentity {
        self.identity
    }

    #[must_use]
    pub const fn book(&self) -> &SaleBook {
        &self.book
    }

    /// # Errors
    /// [`TerminalError::UnknownSale`] if there is no such sale.
    pub fn sale(&self, sale_id: Uuid) -> Result<&Sale, TerminalError> {
        self.book
            .get(sale_id)
            .ok_or(TerminalError::UnknownSale { sale_id })
    }

    /// How many events are waiting to be pushed — drives the "unsynced" badge.
    ///
    /// # Errors
    /// [`TerminalError::Store`] on failure.
    pub fn unsynced_count(&self) -> Result<u64, TerminalError> {
        Ok(self.store.unsynced_count()?)
    }

    /// Takings so far, for the shift banner.
    ///
    /// # Errors
    /// [`TerminalError::Sale`] on overflow.
    pub fn takings(&self, currency: sahl_core::Currency) -> Result<Money, TerminalError> {
        Ok(self.book.takings(currency)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sahl_core::money::{Currency, Rounding};
    use sahl_core::quantity::Quantity;
    use sahl_core::sale::TenderMethod;
    use sahl_core::tax::{PricingMode, TaxClass};

    const BDT: Currency = Currency::Bdt;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn identity() -> DeviceIdentity {
        DeviceIdentity {
            tenant_id: id(1),
            outlet_id: id(2),
            device_id: id(3),
        }
    }

    fn fresh() -> Terminal {
        Terminal::load(
            EventStore::open_in_memory(id(3)).expect("opens"),
            identity(),
        )
        .expect("loads")
    }

    fn at(n: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + n)
    }

    const SALE: u128 = 0x5A1E;

    fn opened() -> SaleEvent {
        SaleEvent::Opened {
            sale_id: id(SALE),
            opened_by: id(0xCA51),
            currency: BDT,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        }
    }

    fn line(minor: i64) -> SaleEvent {
        SaleEvent::LineAdded {
            sale_id: id(SALE),
            line_id: id(11),
            product_id: id(12),
            name: "Rice 5kg".to_owned(),
            unit_price: Money::from_minor(minor, BDT),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
        }
    }

    #[test]
    fn a_fresh_terminal_is_empty() {
        let terminal = fresh();
        assert!(terminal.book().is_empty());
        assert_eq!(terminal.unsynced_count().expect("counts"), 0);
    }

    #[test]
    fn recording_persists_and_projects_together() {
        let mut terminal = fresh();
        terminal.record(&opened(), id(100), at(0)).expect("opens");
        terminal
            .record(&line(48_000), id(101), at(1))
            .expect("adds");

        assert_eq!(terminal.book().len(), 1);
        assert_eq!(terminal.unsynced_count().expect("counts"), 2);
        assert_eq!(
            terminal
                .sale(id(SALE))
                .expect("sale")
                .totals()
                .expect("totals")
                .total,
            Money::from_minor(48_000, BDT)
        );
    }

    #[test]
    fn a_rejected_event_leaves_the_till_exactly_as_it_was() {
        // The reason validation runs against a copy: a refused action must not half-apply, or the
        // UI would have to unwind state it never fully committed.
        let mut terminal = fresh();
        terminal.record(&opened(), id(100), at(0)).expect("opens");
        terminal
            .record(&line(48_000), id(101), at(1))
            .expect("adds");

        let before = terminal.book().fingerprint().expect("canonical");
        let unsynced_before = terminal.unsynced_count().expect("counts");

        // Same line id twice — refused by the aggregate.
        let duplicate = terminal.record(&line(48_000), id(102), at(2));
        assert!(duplicate.is_err());

        assert_eq!(terminal.book().fingerprint().expect("canonical"), before);
        assert_eq!(
            terminal.unsynced_count().expect("counts"),
            unsynced_before,
            "a refused event must not reach the log"
        );
    }

    #[test]
    fn a_terminal_reloads_to_the_same_state() {
        // The crash-recovery path: everything is rebuilt from disk, and must land byte-identically.
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut terminal = Terminal::load(store, identity()).expect("loads");

        terminal.record(&opened(), id(100), at(0)).expect("opens");
        terminal
            .record(&line(48_000), id(101), at(1))
            .expect("adds");
        terminal
            .record(
                &SaleEvent::TenderRecorded {
                    sale_id: id(SALE),
                    tender_id: id(13),
                    method: TenderMethod::Cash,
                    amount: Money::from_minor(48_000, BDT),
                    reference: None,
                },
                id(102),
                at(2),
            )
            .expect("tenders");
        terminal
            .record(
                &SaleEvent::Completed {
                    sale_id: id(SALE),
                    total: Money::from_minor(48_000, BDT),
                    change_given: Money::from_minor(0, BDT),
                },
                id(103),
                at(3),
            )
            .expect("completes");

        let before = terminal.book().fingerprint().expect("canonical");

        // Rebuild from the same events, as a restart would.
        let events = terminal.store.load_all().expect("loads");
        let mut rebuilt = SaleBook::new();
        for envelope in &events {
            if let Ok(event) = envelope.payload_as::<SaleEvent>() {
                rebuilt.apply(&event).expect("valid");
            }
        }

        assert_eq!(rebuilt.fingerprint().expect("canonical"), before);
        assert_eq!(rebuilt.takings(BDT), Ok(Money::from_minor(48_000, BDT)));
    }

    #[test]
    fn a_tampered_log_is_refused_at_load() {
        // Catching this at startup is the point: the device must not write new events on top of a
        // chain that no longer holds.
        let mut store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut chain = EventChain::new(id(3));
        let header = EventHeader {
            event_id: id(100),
            tenant_id: id(1),
            outlet_id: id(2),
            device_id: id(3),
            occurred_at: at(0),
        };
        let mut envelope = chain.append(header, &opened()).expect("seals");
        envelope.hash = sahl_core::EventHash::digest(b"forged");
        store.append(&envelope).expect("stores");

        assert!(matches!(
            Terminal::load(store, identity()),
            Err(TerminalError::CorruptLog { .. })
        ));
    }
}
