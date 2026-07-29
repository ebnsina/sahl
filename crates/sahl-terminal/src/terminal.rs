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

use std::collections::BTreeMap;

use sahl_core::catalogue::{Catalogue, CatalogueError, CatalogueEvent};
use sahl_core::event::{EventChain, EventEnvelope, EventHeader};
use sahl_core::inventory::{InventoryBook, InventoryError, InventoryEvent};
use sahl_core::ledger::{FiscalChain, FiscalEvent, FiscalTip, InvoiceContent};
use sahl_core::outlet::{OutletConfig, OutletError, OutletEvent};
use sahl_core::policy::lease::ClaimVerdict;
use sahl_core::projection::SaleBook;
use sahl_core::purchasing::{PurchaseError, PurchaseEvent, PurchaseOrder};
use sahl_core::sale::{Sale, SaleError, SaleEvent};
use sahl_core::shift::{Shift, ShiftError, ShiftEvent, ShiftReport, ShiftStatus};
use sahl_core::staff::{Directory, DirectoryError, Permission, SignIn, StaffEvent};
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
    Shift(#[from] ShiftError),

    #[error("{0}")]
    Inventory(#[from] InventoryError),

    #[error("{0}")]
    Directory(#[from] DirectoryError),

    #[error("{0}")]
    Purchase(#[from] PurchaseError),

    #[error("{0}")]
    Fiscal(#[from] sahl_core::ledger::FiscalError),

    #[error("{0}")]
    Outlet(#[from] OutletError),

    #[error("{0}")]
    Catalogue(#[from] CatalogueError),

    #[error("{0}")]
    FiscalDocument(#[from] sahl_fiscal::FiscalError),

    #[error("sale {sale_id} has not been invoiced")]
    NotInvoiced { sale_id: Uuid },

    #[error("no purchase order {order_id}")]
    UnknownOrder { order_id: Uuid },

    /// Nobody signed in, or the PIN did not match. Deliberately one variant: the UI shows the same
    /// message either way, and splitting it invites a screen that leaks which half was wrong.
    #[error("that PIN was not accepted")]
    NotAuthorized,

    /// The action needs approval and no active account holds it.
    #[error("nobody at this outlet can approve that")]
    NoApprover,

    #[error("no shift is open")]
    NoOpenShift,

    /// A second shift cannot start while one is running on this device — the drawer is physical and
    /// there is only one of it.
    #[error("a shift is already open on this till")]
    ShiftAlreadyOpen,

    #[error("{0}")]
    Store(#[from] StoreError),

    #[error("event log error: {0}")]
    Event(#[from] sahl_core::event::EventError),

    #[error("no sale {sale_id}")]
    UnknownSale { sale_id: Uuid },

    /// Another device holds this ticket and is still working on it.
    #[error("ticket {sale_id} is held by another device")]
    TicketHeld { sale_id: Uuid, holder: Uuid },

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
    /// The shift running on this till, if any. One drawer, so at most one.
    shift: Option<Shift>,
    stock: InventoryBook,
    staff: Directory,
    catalogue: Catalogue,
    /// The fiscal sequence. Separate from the event chain: that one proves the record of what
    /// happened is intact, this one proves the sequence of invoices is.
    fiscal: FiscalChain,
    /// How this outlet trades. `None` until someone completes setup — a till can ring sales before
    /// it is configured, it just cannot issue fiscal documents for them.
    outlet: Option<OutletConfig>,
    /// Several orders are open at once, unlike the single drawer — so a map, not an Option.
    orders: BTreeMap<Uuid, PurchaseOrder>,
    identity: DeviceIdentity,
}

impl Terminal {
    /// Load a terminal from its store, rebuilding the projections and verifying the chain.
    ///
    /// Two different reads, deliberately. **Verification** runs over this device's own events,
    /// because the hash chain is per device and a merged stream would not verify. **Projections**
    /// are built from every event including siblings', because a shop's takings and open tickets
    /// are the outlet's, not one till's.
    ///
    /// Getting that split wrong is quiet and expensive: rebuilding projections from local events
    /// only means a restart silently drops every sale pulled from another till, and the sync cursor
    /// has already moved past them so they never come back.
    ///
    /// Verification on load is itself deliberate. It costs a pass over the log at startup and it is
    /// the only moment a tampered local database is cheap to catch — before the device starts
    /// writing new events on top of a chain that no longer holds.
    ///
    /// # Errors
    /// [`TerminalError::CorruptLog`] if the stored chain does not verify.
    pub fn load(store: EventStore, identity: DeviceIdentity) -> Result<Self, TerminalError> {
        let own = store.load_all()?;

        sahl_core::event::verify_chain_from_genesis(&own).map_err(|error| {
            TerminalError::CorruptLog {
                reason: error.to_string(),
            }
        })?;

        let stored = store.load_projection_input()?;

        let mut book = SaleBook::new();
        let mut stock = InventoryBook::new();
        let mut staff = Directory::new();
        let mut catalogue = Catalogue::new();
        let mut purchase_events: BTreeMap<Uuid, Vec<PurchaseEvent>> = BTreeMap::new();
        let mut fiscal_tip = FiscalTip::GENESIS;
        let mut outlet: Option<OutletConfig> = None;
        let mut shift_events: Vec<ShiftEvent> = Vec::new();
        for envelope in &stored {
            // Sale and shift events project separately; anything else belongs to a projection this
            // build does not have and is skipped rather than treated as an error.
            match envelope.kind.as_str() {
                kind if kind.starts_with("sale.") => {
                    if let Ok(event) = envelope.payload_as::<SaleEvent>() {
                        // A sibling's history can reach this store mid-sale, so a partial ticket is
                        // expected rather than corrupt — skip what does not apply and keep the rest,
                        // matching what the sync path does after a pull.
                        book.apply(&event).ok();
                    }
                }
                kind if kind.starts_with("shift.") => {
                    if let Ok(event) = envelope.payload_as::<ShiftEvent>() {
                        shift_events.push(event);
                    }
                }
                kind if kind.starts_with("inventory.") => {
                    if let Ok(event) = envelope.payload_as::<InventoryEvent>() {
                        stock.apply(&event)?;
                    }
                }
                kind if kind.starts_with("catalogue.") => {
                    if let Ok(event) = envelope.payload_as::<CatalogueEvent>() {
                        // A sibling's catalogue history can arrive out of order relative to this
                        // device's, so a duplicate or an edit to something not yet seen is expected
                        // rather than corrupt — the same posture the sale projection takes.
                        catalogue.apply(&event).ok();
                    }
                }
                kind if kind.starts_with("staff.") => {
                    if let Ok(event) = envelope.payload_as::<StaffEvent>() {
                        staff.apply(&event)?;
                    }
                }
                kind if kind.starts_with("outlet.") => {
                    if let Ok(event) = envelope.payload_as::<OutletEvent>() {
                        // Last one wins: settings are a full replacement, and these arrive from a
                        // dashboard that may be hours ahead of a till that was offline. An invalid
                        // one is skipped rather than refused — better to keep the last good setup
                        // than to refuse to boot over a bad edit made somewhere else.
                        if let Ok(config) = event.to_config() {
                            outlet = Some(config);
                        }
                    }
                }
                kind if kind.starts_with("fiscal.") => {
                    if let Ok(FiscalEvent::InvoiceIssued { seal, .. }) =
                        envelope.payload_as::<FiscalEvent>()
                    {
                        // Only this device's own invoices advance its counter. A sibling's arrive
                        // through sync and belong to that device's sequence, not this one's.
                        if seal.device_id == identity.device_id {
                            fiscal_tip = FiscalTip {
                                counter: seal.counter,
                                hash: seal.hash,
                            };
                        }
                    }
                }
                kind if kind.starts_with("purchase.") => {
                    if let Ok(event) = envelope.payload_as::<PurchaseEvent>() {
                        // Grouped by order before replay: each aggregate expects its own stream
                        // beginning with a placement, and a merged one fails on the second.
                        purchase_events
                            .entry(event.order_id())
                            .or_default()
                            .push(event);
                    }
                }
                _ => {}
            }
        }

        let chain = EventChain::resume(identity.device_id, store.tip()?);
        Ok(Self {
            store,
            chain,
            book,
            shift: latest_open_shift(&shift_events)?,
            stock,
            staff,
            catalogue,
            fiscal: FiscalChain::resume(identity.device_id, fiscal_tip),
            outlet,
            orders: purchase_events
                .into_iter()
                .filter_map(|(order_id, events)| {
                    // A stream missing its placement belongs to an order this device never saw
                    // opened; skipping it beats refusing to boot over a sibling's history.
                    PurchaseOrder::replay(&events)
                        .ok()
                        .map(|order| (order_id, order))
                })
                .collect(),
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
        // 0. Refuse to write to a ticket another device is actively holding.
        //
        //    Checked here rather than in the aggregate on purpose: replay must accept whatever
        //    actually happened, including a contest that should never have occurred. This is the
        //    layer that knows which device is acting, so it is the layer that can decline.
        self.assert_may_write(event, occurred_at)?;

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

    /// Validate, seal, persist, and project one shift event.
    ///
    /// Same four steps in the same order as [`Terminal::record`], for the same reason — a till that
    /// shows a drawer count it did not persist is worse than one that refuses the count.
    ///
    /// # Errors
    /// [`TerminalError`] if the event is invalid for the current state or cannot be persisted.
    pub fn record_shift(
        &mut self,
        event: &ShiftEvent,
        event_id: Uuid,
        occurred_at: Timestamp,
    ) -> Result<EventEnvelope, TerminalError> {
        let running = self
            .shift
            .as_ref()
            .filter(|shift| shift.status() == ShiftStatus::Open);

        // Validated against a throwaway copy first, so a rejected action never reaches the log.
        let candidate = match (running, event) {
            (Some(_), ShiftEvent::Opened { .. }) => return Err(TerminalError::ShiftAlreadyOpen),
            (Some(shift), _) => {
                let mut candidate = shift.clone();
                candidate.apply(event)?;
                candidate
            }
            // A closed shift is history; opening the next one starts a fresh aggregate rather than
            // reviving it, which is what keeps one drawer session from bleeding into the next.
            (None, ShiftEvent::Opened { .. }) => Shift::replay(std::slice::from_ref(event))?,
            (None, _) => return Err(TerminalError::NoOpenShift),
        };

        let envelope = self.seal(event, event_id, occurred_at)?;
        self.shift = Some(candidate);
        Ok(envelope)
    }

    const fn header(&self, event_id: Uuid, occurred_at: Timestamp) -> EventHeader {
        EventHeader {
            event_id,
            tenant_id: self.identity.tenant_id,
            outlet_id: self.identity.outlet_id,
            device_id: self.identity.device_id,
            occurred_at,
        }
    }

    /// Seal and persist any event without touching a projection.
    ///
    /// Generic over the payload because the chain does not care what kind an event is — it only
    /// hashes the kind string and the canonical bytes. Each family validates against its own
    /// aggregate first and calls this last.
    fn seal<P>(
        &mut self,
        event: &P,
        event_id: Uuid,
        occurred_at: Timestamp,
    ) -> Result<EventEnvelope, TerminalError>
    where
        P: sahl_core::event::EventPayload + Serialize,
    {
        let header = EventHeader {
            event_id,
            tenant_id: self.identity.tenant_id,
            outlet_id: self.identity.outlet_id,
            device_id: self.identity.device_id,
            occurred_at,
        };
        let envelope = self.chain.append(header, event)?;
        self.store.append(&envelope)?;
        Ok(envelope)
    }

    /// Validate, seal, persist, and project one inventory event.
    ///
    /// # Errors
    /// [`TerminalError`] if the event is invalid for the current state or cannot be persisted.
    pub fn record_stock(
        &mut self,
        event: &InventoryEvent,
        event_id: Uuid,
        occurred_at: Timestamp,
    ) -> Result<EventEnvelope, TerminalError> {
        let mut candidate = self.stock.clone();
        candidate.apply(event)?;

        let envelope = self.seal(event, event_id, occurred_at)?;
        self.stock = candidate;
        Ok(envelope)
    }

    /// Validate, seal, persist, and project one purchase event.
    ///
    /// # Errors
    /// [`TerminalError`] if the event is invalid for the current state or cannot be persisted.
    pub fn record_purchase(
        &mut self,
        event: &PurchaseEvent,
        event_id: Uuid,
        occurred_at: Timestamp,
    ) -> Result<EventEnvelope, TerminalError> {
        let order_id = event.order_id();

        let candidate = match self.orders.get(&order_id) {
            Some(order) => {
                let mut candidate = order.clone();
                candidate.apply(event)?;
                candidate
            }
            None => PurchaseOrder::replay(std::slice::from_ref(event))?,
        };

        let envelope = self.seal(event, event_id, occurred_at)?;
        self.orders.insert(order_id, candidate);
        Ok(envelope)
    }

    /// Complete a sale and issue its invoice, in one atomic write.
    ///
    /// Two events because they answer to different authorities: `sale.completed` is the shop's
    /// record of a transaction, `fiscal.invoice_issued` is the state's record of an invoice. They
    /// must land together — a completed sale with no invoice number is unaccounted for, and an
    /// invoice number with no sale is a gap someone has to explain.
    ///
    /// # Errors
    /// [`TerminalError`] if the sale cannot be completed or the write fails. Nothing is written and
    /// neither projection moves when this returns an error.
    pub fn complete_sale(
        &mut self,
        event: &SaleEvent,
        regime: &str,
        issued_by: Uuid,
        occurred_at: Timestamp,
    ) -> Result<sahl_core::ledger::InvoiceSeal, TerminalError> {
        self.assert_may_write(event, occurred_at)?;

        let mut candidate = self.book.clone();
        candidate.apply(event)?;

        // Totals come from the candidate, after completion — the invoice must record what the sale
        // settled at, not what it looked like a moment before.
        let totals = candidate
            .get(event.sale_id())
            .ok_or(TerminalError::UnknownSale {
                sale_id: event.sale_id(),
            })?
            .totals()?;

        let content = InvoiceContent {
            totals,
            regime: regime.to_owned(),
        };

        // Sealed against a copy of the chain, so a failed write cannot burn an invoice number —
        // a gap in the fiscal sequence is exactly what the counter exists to make impossible.
        let mut chain = self.fiscal.clone();
        let seal = chain.seal(event.sale_id(), occurred_at, &content)?;

        let fiscal = FiscalEvent::InvoiceIssued {
            seal: seal.clone(),
            content,
            at: occurred_at,
            issued_by,
        };

        let sale_envelope = self
            .chain
            .append(self.header(Uuid::now_v7(), occurred_at), event)?;
        let fiscal_envelope = self
            .chain
            .append(self.header(Uuid::now_v7(), occurred_at), &fiscal)?;

        self.store.append_all(&[sale_envelope, fiscal_envelope])?;

        self.book = candidate;
        self.fiscal = chain;
        Ok(seal)
    }

    /// Validate, seal, persist, and project one outlet event.
    ///
    /// # Errors
    /// [`TerminalError`] if the settings would not be valid to trade under, or the write fails.
    pub fn record_outlet(
        &mut self,
        event: &OutletEvent,
        event_id: Uuid,
        occurred_at: Timestamp,
    ) -> Result<(), TerminalError> {
        // Validated before it is written, so an outlet can never be left in a state it cannot
        // issue documents under.
        let config = event.to_config()?;

        self.seal(event, event_id, occurred_at)?;
        self.outlet = Some(config);
        Ok(())
    }

    /// How this outlet trades, once it has been set up.
    #[must_use]
    pub const fn outlet(&self) -> Option<&OutletConfig> {
        self.outlet.as_ref()
    }

    /// The regime to issue under, or `none` while the outlet is unconfigured.
    #[must_use]
    pub fn regime(&self) -> &'static str {
        self.outlet
            .as_ref()
            .map_or("none", |outlet| outlet.regime.label())
    }

    /// Rebuild the fiscal document for a completed sale.
    ///
    /// Derived rather than stored. The challan is a *rendering* of facts that already exist — the
    /// sale's lines, the seal's number and time, the outlet's registration — and storing a second
    /// copy would create something that can disagree with all three. It also means a challan can
    /// be reprinted in another language later without the original having pinned the wording.
    ///
    /// # Errors
    /// [`TerminalError`] if the sale is unknown or never issued an invoice, or if the outlet's
    /// configuration cannot produce a valid document.
    pub fn fiscal_document(&self, sale_id: Uuid) -> Result<sahl_fiscal::Document, TerminalError> {
        use sahl_fiscal::{Buyer, FiscalLine, Fiscalization, Invoice, Seller};

        let sale = self.sale(sale_id)?;
        let seal = self
            .invoice_seal(sale_id)?
            .ok_or(TerminalError::NotInvoiced { sale_id })?;

        let Some(outlet) = self.outlet.as_ref() else {
            // An unconfigured outlet trades under no regime, which is a real deployment: the
            // customer gets a receipt and the state is owed nothing extra.
            return Ok(sahl_fiscal::Document::None);
        };

        let invoice = Invoice {
            sale_id,
            sequence: seal.counter,
            issued_at: seal.issued_at,
            seller: Seller {
                name: outlet.name.clone(),
                registration: outlet.tax_registration.clone().unwrap_or_default(),
                address: outlet.address.clone(),
            },
            // Not captured at the counter yet. Above Rule 40(1)'s threshold the document layer
            // refuses rather than issuing blank, which is the correct failure until it is.
            buyer: Buyer::default(),
            lines: sale
                .active_lines()
                .map(|line| FiscalLine {
                    description: line.name.clone(),
                    // The Mushak "Unit of Supply" column. Taken from the catalogue, and falling
                    // back to pieces only for a product this device has never seen — which is a
                    // sibling's sale arriving before its catalogue entry, not a normal sale.
                    unit: self.catalogue.get(line.product_id).map_or_else(
                        || "pcs".to_owned(),
                        |product| product.unit.label().to_owned(),
                    ),
                    quantity_milli: line.quantity.milli(),
                })
                .collect(),
            totals: sale.totals()?,
            destination: None,
        };

        Ok(match outlet.regime {
            sahl_core::outlet::FiscalRegime::BdMushak => {
                sahl_fiscal::bd_mushak::BdMushak.issue(&invoice)?
            }
            _ => sahl_fiscal::noop::NoFiscalRegime.issue(&invoice)?,
        })
    }

    /// The seal a sale was invoiced under, if it has been completed.
    ///
    /// # Errors
    /// [`TerminalError::Store`] if the log cannot be read.
    pub fn invoice_seal(
        &self,
        sale_id: Uuid,
    ) -> Result<Option<sahl_core::ledger::InvoiceSeal>, TerminalError> {
        for envelope in self.store.load_all()? {
            if envelope.kind != "fiscal.invoice_issued" {
                continue;
            }
            if let Ok(FiscalEvent::InvoiceIssued { seal, .. }) =
                envelope.payload_as::<FiscalEvent>()
                && seal.sale_id == sale_id
            {
                return Ok(Some(seal));
            }
        }
        Ok(None)
    }

    /// Build the receipt for a completed sale.
    ///
    /// `printed_at` arrives pre-formatted because a receipt shows local time and only the caller
    /// knows the outlet's timezone — the same reason `sahl-escpos` refuses to format it itself.
    ///
    /// # Errors
    /// [`TerminalError`] if the sale is unknown or was never invoiced.
    pub fn receipt(
        &self,
        sale_id: Uuid,
        printed_at: String,
    ) -> Result<sahl_escpos::ReceiptData, TerminalError> {
        use sahl_escpos::{ReceiptData, ReceiptLine, ReceiptTaxGroup};

        let sale = self.sale(sale_id)?;
        let totals = sale.totals()?;
        let seal = self.invoice_seal(sale_id)?;

        Ok(ReceiptData {
            shop_name: self
                .outlet
                .as_ref()
                .map_or_else(|| "Sahl".to_owned(), |outlet| outlet.name.clone()),
            shop_address: self.outlet.as_ref().map(|outlet| outlet.address.clone()),
            tax_registration: self
                .outlet
                .as_ref()
                .and_then(|outlet| outlet.tax_registration.clone()),
            // The fiscal counter when there is one. Falling back to the sale id gives a customer
            // something to quote back, without pretending it is an invoice number.
            invoice_number: seal
                .map_or_else(|| sale_id.to_string(), |seal| seal.counter.to_string()),
            printed_at,
            currency_label: totals.total.currency().code().to_owned(),
            lines: sale
                .lines()
                .iter()
                .map(|line| ReceiptLine {
                    name: line.name.clone(),
                    quantity: line.quantity,
                    unit_price: line.unit_price,
                    // A voided line contributes nothing, and printing its original value beside a
                    // "VOID" mark is how a customer ends up adding it into the total themselves.
                    total: if line.is_active() {
                        line.unit_price
                    } else {
                        sahl_core::Money::from_minor(0, totals.total.currency())
                    },
                    voided: !line.is_active(),
                })
                .collect(),
            tax_groups: totals
                .tax_groups
                .iter()
                .map(|group| ReceiptTaxGroup {
                    label: tax_group_label(group.tax_class),
                    taxable_base: group.taxable_base,
                    tax: group.tax,
                })
                .collect(),
            discount: (!totals.discount.is_zero()).then_some(totals.discount),
            net: totals.net,
            tax: totals.tax,
            total: totals.total,
            tenders: sale
                .tenders()
                .iter()
                .map(|tender| (tender_label(tender.method), tender.amount))
                .collect(),
            change: sale.change_due().ok().filter(|change| !change.is_zero()),
            footer: None,
        })
    }

    /// Where this device's fiscal sequence has got to.
    #[must_use]
    pub const fn fiscal_tip(&self) -> FiscalTip {
        self.fiscal.tip()
    }

    /// Book a delivery in against an order, in one atomic write.
    ///
    /// Two events, one action: the order records that stock arrived, the batch ledger records it on
    /// the shelf. Appending them separately can leave an order claiming a delivery with no batch to
    /// show for it — and both halves look internally consistent afterwards, so nobody can explain
    /// the discrepancy later.
    ///
    /// # Errors
    /// [`TerminalError`] if either event is invalid or the write fails. Nothing is written and
    /// neither projection moves when this returns an error.
    pub fn record_receipt(
        &mut self,
        purchase: &PurchaseEvent,
        stock: &InventoryEvent,
        occurred_at: Timestamp,
    ) -> Result<(), TerminalError> {
        // Both validated before either is sealed, so a refused receipt leaves the till untouched.
        let order_id = purchase.order_id();
        let mut order = match self.orders.get(&order_id) {
            Some(existing) => existing.clone(),
            None => return Err(TerminalError::UnknownOrder { order_id }),
        };
        order.apply(purchase)?;

        let mut book = self.stock.clone();
        book.apply(stock)?;

        let purchase_envelope = self
            .chain
            .append(self.header(Uuid::now_v7(), occurred_at), purchase)?;
        let stock_envelope = self
            .chain
            .append(self.header(Uuid::now_v7(), occurred_at), stock)?;

        self.store
            .append_all(&[purchase_envelope, stock_envelope])?;

        self.orders.insert(order_id, order);
        self.stock = book;
        Ok(())
    }

    /// Every purchase order this outlet knows about, oldest first.
    #[must_use]
    pub fn orders(&self) -> Vec<&PurchaseOrder> {
        self.orders.values().collect()
    }

    /// # Errors
    /// [`TerminalError::UnknownOrder`] if there is no such order.
    pub fn order(&self, order_id: Uuid) -> Result<&PurchaseOrder, TerminalError> {
        self.orders
            .get(&order_id)
            .ok_or(TerminalError::UnknownOrder { order_id })
    }

    /// Validate, seal, persist, and project one staff event.
    ///
    /// # Errors
    /// [`TerminalError`] if the event is invalid for the current state or cannot be persisted.
    pub fn record_staff(
        &mut self,
        event: &StaffEvent,
        event_id: Uuid,
        occurred_at: Timestamp,
    ) -> Result<EventEnvelope, TerminalError> {
        let mut candidate = self.staff.clone();
        candidate.apply(event)?;

        let envelope = self.seal(event, event_id, occurred_at)?;
        self.staff = candidate;
        Ok(envelope)
    }

    /// The auditable actions in this till's log, with the person responsible for each.
    ///
    /// Actor has to be reconstructed rather than read off the event. Only some events name who
    /// acted — a void records who *approved* it, not who rang it — so a sale's actor is the cashier
    /// who opened it and a movement's is whoever took the till. That is the accurate attribution,
    /// and inventing an `actor` field on every event to avoid this walk would record the same fact
    /// twice and let the two disagree.
    ///
    /// # Errors
    /// [`TerminalError::Store`] if the log cannot be read.
    pub fn audit_entries(&self) -> Result<Vec<sahl_core::staff::AuditEntry>, TerminalError> {
        use std::collections::BTreeMap;

        let stored = self.store.load_all()?;
        let mut sale_actors: BTreeMap<Uuid, Uuid> = BTreeMap::new();
        let mut shift_actors: BTreeMap<Uuid, Uuid> = BTreeMap::new();
        let mut sales = Vec::new();
        let mut shifts = Vec::new();

        for envelope in &stored {
            match envelope.kind.as_str() {
                kind if kind.starts_with("sale.") => {
                    let Ok(event) = envelope.payload_as::<SaleEvent>() else {
                        continue;
                    };
                    if let SaleEvent::Opened {
                        sale_id, opened_by, ..
                    } = &event
                    {
                        sale_actors.insert(*sale_id, *opened_by);
                    }
                    // A sale whose opening never reached this device still happened; attributing it
                    // to nobody is more honest than attributing it to whoever is standing here.
                    let actor = sale_actors
                        .get(&event.sale_id())
                        .copied()
                        .unwrap_or_else(Uuid::nil);
                    sales.push((event, envelope.occurred_at, actor));
                }
                kind if kind.starts_with("shift.") => {
                    let Ok(event) = envelope.payload_as::<ShiftEvent>() else {
                        continue;
                    };
                    if let ShiftEvent::Opened {
                        shift_id,
                        opened_by,
                        ..
                    } = &event
                    {
                        shift_actors.insert(*shift_id, *opened_by);
                    }
                    let actor = shift_actors
                        .get(&event.shift_id())
                        .copied()
                        .unwrap_or_else(Uuid::nil);
                    shifts.push((event, actor));
                }
                _ => {}
            }
        }

        let mut entries = sahl_core::staff::from_sales(&sales);
        entries.extend(sahl_core::staff::from_shifts(&shifts));
        Ok(entries)
    }

    /// Validate, seal, persist, and project one catalogue event.
    ///
    /// # Errors
    /// [`TerminalError`] if the event is invalid for the current state or cannot be persisted.
    pub fn record_catalogue(
        &mut self,
        event: &CatalogueEvent,
        event_id: Uuid,
        occurred_at: Timestamp,
    ) -> Result<(), TerminalError> {
        let mut candidate = self.catalogue.clone();
        candidate.apply(event)?;

        self.seal(event, event_id, occurred_at)?;
        self.catalogue = candidate;
        Ok(())
    }

    /// What this shop sells.
    #[must_use]
    pub const fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }

    /// Who works here.
    #[must_use]
    pub const fn staff(&self) -> &Directory {
        &self.staff
    }

    /// Authenticate someone senior enough to approve `permission`, returning their id.
    ///
    /// The id this returns is what goes into the event's `authorized_by`. That is the whole point:
    /// an approval field filled in by the UI from a constant records nothing, and every control
    /// built on top of it — the audit feed, the self-approval check — is then decorative.
    ///
    /// # Errors
    /// [`TerminalError::NotAuthorized`] on a wrong PIN, [`TerminalError::NoApprover`] when no
    /// active account holds the permission at all.
    pub fn approve(&self, permission: Permission, pin: &str) -> Result<Uuid, TerminalError> {
        match self.staff.approve(permission, pin)? {
            SignIn::Ok { staff_id, .. } => Ok(staff_id),
            SignIn::Unknown => Err(TerminalError::NoApprover),
            SignIn::WrongPin | SignIn::Inactive => Err(TerminalError::NotAuthorized),
        }
    }

    /// Authenticate one named person — the sign-in at the start of a shift.
    ///
    /// # Errors
    /// [`TerminalError::NotAuthorized`] if the PIN does not match or the account is inactive.
    pub fn sign_in(&self, staff_id: Uuid, pin: &str) -> Result<Uuid, TerminalError> {
        match self.staff.sign_in(staff_id, pin)? {
            SignIn::Ok { staff_id, .. } => Ok(staff_id),
            SignIn::Unknown | SignIn::WrongPin | SignIn::Inactive => {
                Err(TerminalError::NotAuthorized)
            }
        }
    }

    /// Every batch this outlet knows about.
    #[must_use]
    pub const fn stock(&self) -> &InventoryBook {
        &self.stock
    }

    /// The shift running on this till, if one is.
    #[must_use]
    pub const fn shift(&self) -> Option<&Shift> {
        self.shift.as_ref()
    }

    /// The X/Z report for the running shift.
    ///
    /// Reads sales from the projection rather than being handed them, so the expected-cash figure
    /// cannot be computed against a different set of sales than the screen is showing.
    ///
    /// # Errors
    /// [`TerminalError::NoOpenShift`] if no shift is running; [`TerminalError::Shift`] on overflow.
    pub fn shift_report(&self) -> Result<ShiftReport, TerminalError> {
        let shift = self.shift.as_ref().ok_or(TerminalError::NoOpenShift)?;
        let sales: Vec<&Sale> = self.book.completed().collect();
        Ok(sahl_core::shift::report(shift, sales)?)
    }

    /// Refuse a write to a ticket held by an active sibling.
    ///
    /// A claim is always allowed through — that is the request to take it, and a contest is settled
    /// by `resolve_contest` once both reach the server.
    fn assert_may_write(&self, event: &SaleEvent, now: Timestamp) -> Result<(), TerminalError> {
        if matches!(
            event,
            SaleEvent::Opened { .. } | SaleEvent::TicketClaimed { .. }
        ) {
            return Ok(());
        }

        let Some(sale) = self.book.get(event.sale_id()) else {
            return Ok(());
        };

        // No lease means nobody claimed it — the ordinary retail case, where a ticket opens and
        // closes on one till and leases never come up at all.
        if let ClaimVerdict::Held { holder } = sale.may_write(self.identity.device_id, now) {
            return Err(TerminalError::TicketHeld {
                sale_id: event.sale_id(),
                holder,
            });
        }
        Ok(())
    }

    /// Run one sync round against this till's store.
    ///
    /// Rebuilds the projection when events arrive from a sibling — a second cashier's sales change
    /// what this screen should show, and a stale projection is how two tills in one shop start
    /// disagreeing about the day.
    ///
    /// # Errors
    /// [`crate::sync::SyncClientError`] on refusal, storage failure, or tip disagreement.
    pub fn sync(
        &mut self,
        transport: &mut impl crate::sync::Transport,
    ) -> Result<crate::sync::SyncOutcome, crate::sync::SyncClientError> {
        let outcome = crate::sync::sync_once(&mut self.store, transport)?;

        if outcome.pulled > 0 {
            let mut rebuilt = SaleBook::new();
            for envelope in &self.store.load_projection_input()? {
                if let Ok(event) = envelope.payload_as::<SaleEvent>() {
                    // A sibling's history may reach us mid-sale, so a partial ticket is expected
                    // rather than corrupt; skip what does not apply and keep the rest.
                    rebuilt.apply(&event).ok();
                }
            }
            self.book = rebuilt;
        }

        Ok(outcome)
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

/// How a tender prints on a receipt.
///
/// Written out rather than derived from `Debug`, which would put `MobileWallet { wallet: Bkash }`
/// on a customer's receipt.
fn tender_label(method: sahl_core::sale::TenderMethod) -> String {
    use sahl_core::sale::TenderMethod as M;
    match method {
        M::Cash => "Cash".to_owned(),
        M::Card => "Card".to_owned(),
        M::MobileWallet { wallet } => format!("{wallet:?}"),
        M::BankTransfer => "Bank transfer".to_owned(),
        M::StoreCredit => "Store credit".to_owned(),
        // TenderMethod is #[non_exhaustive]. A method this build cannot name prints honestly
        // rather than as a Rust debug string a customer would have to decipher.
        _ => "Other".to_owned(),
    }
}

/// How a VAT class prints on a receipt.
///
/// Zero-rated and exempt are named rather than shown as "0%": they are different treatments, and a
/// customer reading a receipt is the last person who should have to infer which.
fn tax_group_label(class: sahl_core::tax::TaxClass) -> String {
    match class {
        sahl_core::tax::TaxClass::Standard { rate } => format!("VAT {rate}"),
        sahl_core::tax::TaxClass::ZeroRated => "Zero-rated".to_owned(),
        sahl_core::tax::TaxClass::Exempt => "Exempt".to_owned(),
    }
}

/// Rebuild the shift a restart should resume.
///
/// Events are grouped by shift id and the last one replayed. Grouping matters because a till that
/// has run for a week holds several closed shifts, and replaying them as one stream would fail on
/// the second `Opened` — a crash mid-shift must reopen the drawer where it was, not refuse to boot.
fn latest_open_shift(events: &[ShiftEvent]) -> Result<Option<Shift>, TerminalError> {
    let Some(latest) = events.last().map(ShiftEvent::shift_id) else {
        return Ok(None);
    };

    let mine: Vec<ShiftEvent> = events
        .iter()
        .filter(|event| event.shift_id() == latest)
        .cloned()
        .collect();

    let shift = Shift::replay(&mine)?;
    // A closed shift is not resumed. The next cashier opens their own, and the closing count of the
    // last one is already settled history.
    Ok((shift.status() == ShiftStatus::Open).then_some(shift))
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

    fn tender(minor: i64) -> SaleEvent {
        SaleEvent::TenderRecorded {
            sale_id: id(SALE),
            tender_id: id(13),
            method: TenderMethod::Cash,
            amount: Money::from_minor(minor, BDT),
            reference: None,
        }
    }

    fn completed(at: Timestamp, total: i64, change_given: i64) -> SaleEvent {
        SaleEvent::Completed {
            sale_id: id(SALE),
            total: Money::from_minor(total, BDT),
            change_given: Money::from_minor(change_given, BDT),
            at,
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
                    at: at(3),
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

    // ------------------------------------------------------------------------------------------
    // Shifts
    // ------------------------------------------------------------------------------------------

    const CASHIER: u128 = 0xCA51;
    const MANAGER: u128 = 0x11A;
    const SHIFT: u128 = 0x581F;

    fn shift_opened() -> ShiftEvent {
        ShiftEvent::Opened {
            shift_id: id(SHIFT),
            opened_by: id(CASHIER),
            currency: BDT,
            opening_float: Money::from_minor(200_000, BDT),
            at: at(0),
        }
    }

    #[test]
    fn a_shift_opens_and_the_report_starts_from_the_float() {
        let mut till = fresh();
        till.record_shift(&shift_opened(), id(90), at(0))
            .expect("opens");

        let report = till.shift_report().expect("reports");
        assert_eq!(report.opening_float, Money::from_minor(200_000, BDT));
        assert_eq!(report.expected_cash, Money::from_minor(200_000, BDT));
        assert!(!report.is_final, "an X report, not a Z");
    }

    #[test]
    fn a_second_shift_cannot_open_over_a_running_one() {
        // There is one physical drawer. Two open sessions would make the expected-cash figure the
        // sum of two people's accountability, which is nobody's.
        let mut till = fresh();
        till.record_shift(&shift_opened(), id(90), at(0))
            .expect("opens");

        let result = till.record_shift(
            &ShiftEvent::Opened {
                shift_id: id(0x582F),
                opened_by: id(MANAGER),
                currency: BDT,
                opening_float: Money::from_minor(100_000, BDT),
                at: at(10),
            },
            id(91),
            at(10),
        );

        assert!(matches!(result, Err(TerminalError::ShiftAlreadyOpen)));
    }

    #[test]
    fn nothing_may_be_recorded_before_a_shift_is_open() {
        let mut till = fresh();
        let result = till.record_shift(
            &ShiftEvent::Counted {
                shift_id: id(SHIFT),
                counted: Money::from_minor(200_000, BDT),
                counted_by: id(CASHIER),
                at: at(1),
            },
            id(90),
            at(1),
        );

        assert!(matches!(result, Err(TerminalError::NoOpenShift)));
    }

    #[test]
    fn cash_sales_land_in_the_expected_drawer_figure() {
        let mut till = fresh();
        till.record_shift(&shift_opened(), id(90), at(0))
            .expect("opens");

        till.record(&opened(), id(80), at(1)).expect("opens sale");
        till.record(&line(50_000), id(81), at(2)).expect("adds");
        till.record(&tender(50_000), id(82), at(3))
            .expect("tenders");
        till.record(&completed(at(4), 50_000, 0), id(83), at(4))
            .expect("completes");

        let report = till.shift_report().expect("reports");
        assert_eq!(report.sale_count, 1);
        assert_eq!(
            report.expected_cash,
            Money::from_minor(250_000, BDT),
            "float plus the cash taken"
        );
    }

    #[test]
    fn a_skim_reduces_what_the_drawer_should_hold() {
        let mut till = fresh();
        till.record_shift(&shift_opened(), id(90), at(0))
            .expect("opens");
        till.record_shift(
            &ShiftEvent::CashMoved {
                shift_id: id(SHIFT),
                movement_id: id(70),
                amount: Money::from_minor(-100_000, BDT),
                reason: sahl_core::shift::CashMovementReason::Skim,
                note: None,
                authorized_by: id(MANAGER),
                at: at(5),
            },
            id(91),
            at(5),
        )
        .expect("moves cash");

        let report = till.shift_report().expect("reports");
        assert_eq!(report.expected_cash, Money::from_minor(100_000, BDT));
    }

    #[test]
    fn a_short_count_is_reported_as_short() {
        let mut till = fresh();
        till.record_shift(&shift_opened(), id(90), at(0))
            .expect("opens");
        till.record_shift(
            &ShiftEvent::Counted {
                shift_id: id(SHIFT),
                counted: Money::from_minor(199_300, BDT),
                counted_by: id(CASHIER),
                at: at(6),
            },
            id(91),
            at(6),
        )
        .expect("counts");

        let report = till.shift_report().expect("reports");
        assert_eq!(
            report.variance,
            Some(sahl_core::shift::Variance::Short {
                by: Money::from_minor(700, BDT)
            })
        );
    }

    #[test]
    fn a_restart_mid_shift_reopens_the_same_drawer() {
        // A crash during a rush must not lose the float or the movements already recorded.
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        till.record_shift(&shift_opened(), id(90), at(0))
            .expect("opens");

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(reloaded.shift().map(Shift::id), Some(id(SHIFT)));
        assert_eq!(
            reloaded.shift_report().expect("reports").opening_float,
            Money::from_minor(200_000, BDT)
        );
    }

    #[test]
    fn a_restart_after_close_does_not_reopen_the_shift() {
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        till.record_shift(&shift_opened(), id(90), at(0))
            .expect("opens");
        // The drawer must be counted before it can be closed — the aggregate refuses otherwise.
        till.record_shift(
            &ShiftEvent::Counted {
                shift_id: id(SHIFT),
                counted: Money::from_minor(200_000, BDT),
                counted_by: id(CASHIER),
                at: at(8),
            },
            id(93),
            at(8),
        )
        .expect("counts");
        till.record_shift(
            &ShiftEvent::Closed {
                shift_id: id(SHIFT),
                closed_by: id(MANAGER),
                closing_cash: Money::from_minor(200_000, BDT),
                at: at(9),
            },
            id(91),
            at(9),
        )
        .expect("closes");

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert!(
            reloaded.shift().is_none(),
            "the next cashier opens their own"
        );
    }

    #[test]
    fn a_second_shift_after_a_close_loads_cleanly() {
        // The bug this guards: replaying a week of shifts as one stream fails on the second
        // `Opened`, and a till that will not boot mid-week is worse than one that loses a report.
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        till.record_shift(&shift_opened(), id(90), at(0))
            .expect("opens");
        // The drawer must be counted before it can be closed — the aggregate refuses otherwise.
        till.record_shift(
            &ShiftEvent::Counted {
                shift_id: id(SHIFT),
                counted: Money::from_minor(200_000, BDT),
                counted_by: id(CASHIER),
                at: at(8),
            },
            id(93),
            at(8),
        )
        .expect("counts");
        till.record_shift(
            &ShiftEvent::Closed {
                shift_id: id(SHIFT),
                closed_by: id(MANAGER),
                closing_cash: Money::from_minor(200_000, BDT),
                at: at(9),
            },
            id(91),
            at(9),
        )
        .expect("closes");
        till.record_shift(
            &ShiftEvent::Opened {
                shift_id: id(0x582F),
                opened_by: id(MANAGER),
                currency: BDT,
                opening_float: Money::from_minor(150_000, BDT),
                at: at(10),
            },
            id(92),
            at(10),
        )
        .expect("opens the next one");

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(reloaded.shift().map(Shift::id), Some(id(0x582F)));
        assert_eq!(
            reloaded.shift_report().expect("reports").opening_float,
            Money::from_minor(150_000, BDT)
        );
    }

    // ------------------------------------------------------------------------------------------
    // Stock
    // ------------------------------------------------------------------------------------------

    const BATCH: u128 = 0xBA7C;
    const RICE: u128 = 0x21;

    fn received(batch: u128, milli: i64) -> InventoryEvent {
        InventoryEvent::BatchReceived {
            batch_id: id(batch),
            product_id: id(RICE),
            lot: Some("KT-4471".to_owned()),
            expires_at: Some(at(90)),
            quantity: sahl_core::quantity::Quantity::from_milli(milli),
            unit_cost: Money::from_minor(4_000, BDT),
            supplier: Some("Karim Traders".to_owned()),
            at: at(0),
            received_by: id(CASHIER),
        }
    }

    #[test]
    fn a_delivery_becomes_a_batch_on_the_till() {
        let mut till = fresh();
        till.record_stock(&received(BATCH, 10_000), id(70), at(0))
            .expect("receives");

        assert_eq!(
            till.stock().level(id(BATCH)).expect("present").on_hand,
            sahl_core::quantity::Quantity::from_milli(10_000)
        );
    }

    #[test]
    fn a_rejected_stock_event_leaves_the_book_exactly_as_it_was() {
        // Same reason as sales: a refused action must not half-apply.
        let mut till = fresh();
        till.record_stock(&received(BATCH, 10_000), id(70), at(0))
            .expect("receives");

        let unsynced_before = till.unsynced_count().expect("counts");
        let duplicate = till.record_stock(&received(BATCH, 5_000), id(71), at(1));

        assert!(duplicate.is_err(), "the same batch cannot arrive twice");
        assert_eq!(
            till.stock().level(id(BATCH)).expect("present").on_hand,
            sahl_core::quantity::Quantity::from_milli(10_000)
        );
        assert_eq!(till.unsynced_count().expect("counts"), unsynced_before);
    }

    #[test]
    fn a_count_that_disagrees_survives_a_restart() {
        // The variance is the shrinkage record. Losing it on reboot would erase the only evidence
        // that stock went missing.
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        till.record_stock(&received(BATCH, 10_000), id(70), at(0))
            .expect("receives");
        till.record_stock(
            &InventoryEvent::BatchCounted {
                batch_id: id(BATCH),
                counted: sahl_core::quantity::Quantity::from_milli(9_400),
                at: at(1),
                counted_by: id(CASHIER),
            },
            id(71),
            at(1),
        )
        .expect("counts");

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(reloaded.stock().variances().len(), 1);
        assert_eq!(
            reloaded.stock().variances()[0].delta,
            sahl_core::quantity::Quantity::from_milli(-600),
            "600g missing"
        );
    }

    #[test]
    fn sales_and_stock_share_one_chain_without_disturbing_each_other() {
        // Both families append to the same hash chain. If either projection consumed the other's
        // events, this is where it would show.
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");

        till.record(&opened(), id(80), at(0)).expect("opens sale");
        till.record_stock(&received(BATCH, 10_000), id(70), at(1))
            .expect("receives");
        till.record(&line(48_000), id(81), at(2)).expect("adds");

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(reloaded.book().len(), 1);
        assert_eq!(reloaded.stock().levels().len(), 1);
    }

    // ------------------------------------------------------------------------------------------
    // Staff and approval
    // ------------------------------------------------------------------------------------------

    fn salt(seed: &str) -> argon2::password_hash::SaltString {
        let padded = format!("{seed}-sahl-test");
        argon2::password_hash::SaltString::encode_b64(padded.as_bytes()).expect("valid salt")
    }

    fn enrolled(who: u128, name: &str, role: sahl_core::staff::Role, secret: &str) -> StaffEvent {
        StaffEvent::Enrolled {
            staff_id: id(who),
            name: name.to_owned(),
            role,
            pin_hash: sahl_core::staff::pin::hash(secret, &salt(name)).expect("hashes"),
            at: at(0),
            enrolled_by: Uuid::nil(),
        }
    }

    fn staffed() -> Terminal {
        let mut till = fresh();
        till.record_staff(
            &enrolled(CASHIER, "Ruma", sahl_core::staff::Role::Cashier, "8317"),
            id(60),
            at(0),
        )
        .expect("enrols");
        till.record_staff(
            &enrolled(MANAGER, "Habib", sahl_core::staff::Role::Manager, "5294"),
            id(61),
            at(1),
        )
        .expect("enrols");
        till
    }

    #[test]
    fn a_manager_pin_yields_the_id_that_goes_into_authorized_by() {
        // The whole point of the wiring: the approver is proved, not asserted by the caller.
        let till = staffed();
        assert_eq!(
            till.approve(Permission::VoidLine, "5294")
                .expect("approves"),
            id(MANAGER)
        );
    }

    #[test]
    fn a_cashier_pin_does_not_authorise_a_void() {
        let till = staffed();
        assert!(matches!(
            till.approve(Permission::VoidLine, "8317"),
            Err(TerminalError::NotAuthorized)
        ));
    }

    #[test]
    fn an_outlet_with_no_manager_says_so_rather_than_blaming_the_pin() {
        // "Nobody can approve that" and "you typed it wrong" need different responses at a counter.
        let mut till = fresh();
        till.record_staff(
            &enrolled(CASHIER, "Ruma", sahl_core::staff::Role::Cashier, "8317"),
            id(60),
            at(0),
        )
        .expect("enrols");

        assert!(matches!(
            till.approve(Permission::VoidLine, "8317"),
            Err(TerminalError::NoApprover)
        ));
    }

    #[test]
    fn staff_survive_a_restart() {
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        till.record_staff(
            &enrolled(MANAGER, "Habib", sahl_core::staff::Role::Manager, "5294"),
            id(60),
            at(0),
        )
        .expect("enrols");

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(
            reloaded
                .approve(Permission::VoidLine, "5294")
                .expect("approves"),
            id(MANAGER)
        );
    }

    #[test]
    fn a_departed_manager_stops_being_able_to_approve() {
        // Someone who left on Friday must not still authorise voids on Monday.
        let mut till = staffed();
        till.record_staff(
            &StaffEvent::Deactivated {
                staff_id: id(MANAGER),
                at: at(10),
                deactivated_by: id(MANAGER),
            },
            id(62),
            at(10),
        )
        .expect("deactivates");

        assert!(matches!(
            till.approve(Permission::VoidLine, "5294"),
            Err(TerminalError::NoApprover)
        ));
    }

    #[test]
    fn the_audit_feed_attributes_a_void_to_the_cashier_and_the_approver_separately() {
        // The event records who approved; the actor has to be reconstructed from who opened the
        // sale. Conflating them would make every void look self-approved.
        let mut till = staffed();
        till.record(&opened(), id(80), at(1)).expect("opens sale");
        till.record(&line(48_000), id(81), at(2)).expect("adds");
        till.record(
            &SaleEvent::LineVoided {
                sale_id: id(SALE),
                line_id: id(11),
                reason: sahl_core::sale::VoidReason::Mistake,
                authorized_by: id(MANAGER),
            },
            id(82),
            at(3),
        )
        .expect("voids");

        let entries = till.audit_entries().expect("reads");
        assert_eq!(entries.len(), 1, "only the void is auditable");
        assert_eq!(entries[0].actor, id(CASHIER), "who rang it");
        assert_eq!(entries[0].approved_by, Some(id(MANAGER)), "who allowed it");
        assert!(!entries[0].is_self_approved());
    }

    #[test]
    fn a_cashier_approving_their_own_void_shows_up_as_unapproved() {
        let mut till = staffed();
        till.record(&opened(), id(80), at(1)).expect("opens sale");
        till.record(&line(48_000), id(81), at(2)).expect("adds");
        till.record(
            &SaleEvent::LineVoided {
                sale_id: id(SALE),
                line_id: id(11),
                reason: sahl_core::sale::VoidReason::Mistake,
                authorized_by: id(CASHIER),
            },
            id(82),
            at(3),
        )
        .expect("voids");

        let entries = till.audit_entries().expect("reads");
        let flagged = sahl_core::staff::unapproved(&entries, |actor| till.staff().role_of(actor));

        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0].actor, id(CASHIER));
    }

    // ------------------------------------------------------------------------------------------
    // Purchase orders
    // ------------------------------------------------------------------------------------------

    const ORDER: u128 = 0x0DE2;
    const ORDER_LINE: u128 = 0x11E;

    fn placed(order: u128, milli: i64, cost: i64) -> PurchaseEvent {
        PurchaseEvent::Placed {
            order_id: id(order),
            supplier: "Karim Traders".to_owned(),
            reference: Some("KT-4471".to_owned()),
            lines: vec![sahl_core::purchasing::OrderLine {
                line_id: id(ORDER_LINE),
                product_id: id(RICE),
                quantity: sahl_core::quantity::Quantity::from_milli(milli),
                unit_cost: Money::from_minor(cost, BDT),
            }],
            expected_at: Some(at(3)),
            at: at(0),
            placed_by: id(CASHIER),
        }
    }

    fn receipt(order: u128, batch: u128, milli: i64, cost: i64) -> (PurchaseEvent, InventoryEvent) {
        let quantity = sahl_core::quantity::Quantity::from_milli(milli);
        let unit_cost = Money::from_minor(cost, BDT);
        (
            PurchaseEvent::LineReceived {
                order_id: id(order),
                line_id: id(ORDER_LINE),
                batch_id: id(batch),
                quantity,
                unit_cost,
                at: at(3),
                received_by: id(CASHIER),
            },
            InventoryEvent::BatchReceived {
                batch_id: id(batch),
                product_id: id(RICE),
                lot: Some("KT-4471".to_owned()),
                expires_at: Some(at(90)),
                quantity,
                unit_cost,
                supplier: Some("Karim Traders".to_owned()),
                at: at(3),
                received_by: id(CASHIER),
            },
        )
    }

    #[test]
    fn a_receipt_moves_the_order_and_the_shelf_together() {
        let mut till = fresh();
        till.record_purchase(&placed(ORDER, 50_000, 4_000), id(70), at(0))
            .expect("places");

        let (purchase, stock) = receipt(ORDER, BATCH, 50_000, 4_000);
        till.record_receipt(&purchase, &stock, at(3))
            .expect("receives");

        assert_eq!(
            till.order(id(ORDER))
                .expect("present")
                .line(id(ORDER_LINE))
                .expect("line")
                .received,
            sahl_core::quantity::Quantity::from_milli(50_000)
        );
        assert_eq!(
            till.stock().level(id(BATCH)).expect("present").on_hand,
            sahl_core::quantity::Quantity::from_milli(50_000)
        );
    }

    #[test]
    fn a_refused_receipt_writes_neither_half() {
        // The reason record_receipt exists. An order claiming a delivery with no batch to show for
        // it looks internally consistent on both sides, so nobody can explain it afterwards.
        let mut till = fresh();
        till.record_purchase(&placed(ORDER, 50_000, 4_000), id(70), at(0))
            .expect("places");
        till.record_purchase(
            &PurchaseEvent::Closed {
                order_id: id(ORDER),
                reason: sahl_core::purchasing::CloseReason::Cancelled,
                at: at(2),
                closed_by: id(CASHIER),
            },
            id(71),
            at(2),
        )
        .expect("closes");

        let unsynced_before = till.unsynced_count().expect("counts");
        let (purchase, stock) = receipt(ORDER, BATCH, 50_000, 4_000);
        let refused = till.record_receipt(&purchase, &stock, at(3));

        assert!(refused.is_err(), "a closed order takes no more stock");
        assert!(
            till.stock().level(id(BATCH)).is_none(),
            "nothing reached the shelf"
        );
        assert_eq!(
            till.unsynced_count().expect("counts"),
            unsynced_before,
            "nothing reached the log either"
        );
    }

    #[test]
    fn a_part_delivery_leaves_the_rest_outstanding() {
        let mut till = fresh();
        till.record_purchase(&placed(ORDER, 50_000, 4_000), id(70), at(0))
            .expect("places");

        let (purchase, stock) = receipt(ORDER, BATCH, 30_000, 4_000);
        till.record_receipt(&purchase, &stock, at(3))
            .expect("receives");

        assert_eq!(
            till.order(id(ORDER))
                .expect("present")
                .line(id(ORDER_LINE))
                .expect("line")
                .outstanding(),
            Ok(sahl_core::quantity::Quantity::from_milli(20_000)),
            "20kg short, invisible to the batch ledger alone"
        );
    }

    #[test]
    fn several_orders_survive_a_restart_independently() {
        // The bug this guards: replaying every purchase event as one stream fails on the second
        // placement, exactly as it would have for shifts.
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        till.record_purchase(&placed(ORDER, 50_000, 4_000), id(70), at(0))
            .expect("places");
        till.record_purchase(&placed(0x0DE3, 12_000, 18_000), id(71), at(1))
            .expect("places a second");

        let (purchase, stock) = receipt(ORDER, BATCH, 50_000, 4_000);
        till.record_receipt(&purchase, &stock, at(3))
            .expect("receives");

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(reloaded.orders().len(), 2);
        assert_eq!(
            reloaded
                .order(id(ORDER))
                .expect("present")
                .status()
                .expect("computes"),
            sahl_core::purchasing::OrderStatus::FullyReceived
        );
        assert_eq!(
            reloaded
                .order(id(0x0DE3))
                .expect("present")
                .status()
                .expect("computes"),
            sahl_core::purchasing::OrderStatus::Awaiting
        );
    }

    #[test]
    fn a_price_change_between_quote_and_delivery_survives_a_restart() {
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        till.record_purchase(&placed(ORDER, 50_000, 4_000), id(70), at(0))
            .expect("places");

        let (purchase, stock) = receipt(ORDER, BATCH, 50_000, 4_600);
        till.record_receipt(&purchase, &stock, at(3))
            .expect("receives");

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(
            reloaded
                .order(id(ORDER))
                .expect("present")
                .price_discrepancies()
                .expect("computes")
                .len(),
            1
        );
    }

    #[test]
    fn only_an_owner_can_enrol_anyone() {
        // The reason the first person must be an owner: a shop whose only account is a cashier can
        // never add a second, and there is no way out short of editing the event log by hand.
        let till = staffed();

        assert!(matches!(
            till.approve(Permission::ManageStaff, "5294"),
            Err(TerminalError::NoApprover)
        ));
    }

    #[test]
    fn an_owner_pin_can_enrol() {
        let mut till = fresh();
        till.record_staff(
            &enrolled(0x0E, "Bashir", sahl_core::staff::Role::Owner, "7712"),
            id(60),
            at(0),
        )
        .expect("enrols the first");

        assert_eq!(
            till.approve(Permission::ManageStaff, "7712")
                .expect("approves"),
            id(0x0E)
        );
    }

    // ------------------------------------------------------------------------------------------
    // The fiscal sequence
    // ------------------------------------------------------------------------------------------

    fn ring_up(till: &mut Terminal, base: u128, minor: i64) {
        let sale = id(base);
        till.record(
            &SaleEvent::Opened {
                sale_id: sale,
                opened_by: id(CASHIER),
                currency: BDT,
                pricing_mode: PricingMode::TaxInclusive,
                rounding: Rounding::HalfUp,
            },
            id(base + 1),
            at(0),
        )
        .expect("opens");
        till.record(
            &SaleEvent::LineAdded {
                sale_id: sale,
                line_id: id(base + 2),
                product_id: id(12),
                name: "Rice 5kg".to_owned(),
                unit_price: Money::from_minor(minor, BDT),
                quantity: Quantity::ONE,
                tax_class: TaxClass::standard(1500),
            },
            id(base + 3),
            at(1),
        )
        .expect("adds");
        till.record(
            &SaleEvent::TenderRecorded {
                sale_id: sale,
                tender_id: id(base + 4),
                method: TenderMethod::Cash,
                amount: Money::from_minor(minor, BDT),
                reference: None,
            },
            id(base + 5),
            at(2),
        )
        .expect("tenders");
    }

    fn settle(till: &mut Terminal, base: u128, minor: i64) -> sahl_core::ledger::InvoiceSeal {
        till.complete_sale(
            &SaleEvent::Completed {
                sale_id: id(base),
                total: Money::from_minor(minor, BDT),
                change_given: Money::from_minor(0, BDT),
                at: at(3),
            },
            "bd_mushak",
            id(CASHIER),
            at(3),
        )
        .expect("completes")
    }

    #[test]
    fn completing_a_sale_issues_the_next_invoice_number() {
        let mut till = fresh();
        ring_up(&mut till, 0x100, 11_500);
        let first = settle(&mut till, 0x100, 11_500);

        ring_up(&mut till, 0x200, 34_000);
        let second = settle(&mut till, 0x200, 34_000);

        assert_eq!(first.counter, 1, "invoices start at one");
        assert_eq!(second.counter, 2);
        assert!(first.previous_hash.is_genesis());
        assert_eq!(second.previous_hash, first.hash, "each embeds the last");
    }

    #[test]
    fn a_refused_completion_does_not_burn_an_invoice_number() {
        // The reason the seal is taken against a copy of the chain. A gap in the fiscal sequence is
        // precisely what the counter exists to make impossible, and an inspector cannot tell a
        // burnt number from a deleted sale.
        let mut till = fresh();
        ring_up(&mut till, 0x100, 11_500);
        settle(&mut till, 0x100, 11_500);

        let before = till.fiscal_tip();
        // Completing the same sale twice is refused by the aggregate.
        let refused = till.complete_sale(
            &SaleEvent::Completed {
                sale_id: id(0x100),
                total: Money::from_minor(11_500, BDT),
                change_given: Money::from_minor(0, BDT),
                at: at(4),
            },
            "bd_mushak",
            id(CASHIER),
            at(4),
        );

        assert!(refused.is_err());
        assert_eq!(till.fiscal_tip(), before, "the counter did not move");
    }

    #[test]
    fn the_fiscal_sequence_survives_a_restart() {
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        ring_up(&mut till, 0x100, 11_500);
        let first = settle(&mut till, 0x100, 11_500);

        let (store, _) = till.into_parts();
        let mut reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(reloaded.fiscal_tip().counter, 1);
        ring_up(&mut reloaded, 0x200, 34_000);
        let second = settle(&mut reloaded, 0x200, 34_000);

        assert_eq!(second.counter, 2, "no restart of the sequence");
        assert_eq!(
            second.previous_hash, first.hash,
            "and no break in the chain"
        );
    }

    #[test]
    fn the_invoice_records_what_the_sale_settled_at() {
        // Sealed from the completed sale, not from a snapshot taken before completion.
        let mut till = fresh();
        ring_up(&mut till, 0x100, 11_500);
        settle(&mut till, 0x100, 11_500);

        let stored = till.store.load_all().expect("reads");
        let issued = stored
            .iter()
            .find(|envelope| envelope.kind == "fiscal.invoice_issued")
            .expect("present")
            .payload_as::<sahl_core::ledger::FiscalEvent>()
            .expect("decodes");

        let sahl_core::ledger::FiscalEvent::InvoiceIssued { content, seal, .. } = issued;
        assert_eq!(content.totals.total, Money::from_minor(11_500, BDT));
        assert_eq!(content.regime, "bd_mushak");
        assert_eq!(seal.sale_id, id(0x100));
    }

    #[test]
    fn a_restart_keeps_sales_pulled_from_another_till() {
        // Found by mutation-testing the fiscal guard: startup rebuilt projections from local
        // events only while the sync path used every event, so a restart silently dropped every
        // sibling's sale — and the sync cursor had already moved past them, so they never returned.
        // A two-till shop would under-report its takings from the first reboot onwards.
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        ring_up(&mut till, 0x100, 11_500);
        settle(&mut till, 0x100, 11_500);

        let (mut store, _) = till.into_parts();

        // A sibling's completed sale arrives through sync.
        let sibling = id(0x51B);
        let mut their_events = sahl_core::event::EventChain::new(sibling);
        let their_sale = id(0x700);
        let mut seq = 0_i64;
        for event in [
            SaleEvent::Opened {
                sale_id: their_sale,
                opened_by: id(CASHIER),
                currency: BDT,
                pricing_mode: PricingMode::TaxInclusive,
                rounding: Rounding::HalfUp,
            },
            SaleEvent::LineAdded {
                sale_id: their_sale,
                line_id: id(0x701),
                product_id: id(12),
                name: "Tea 400g".to_owned(),
                unit_price: Money::from_minor(32_000, BDT),
                quantity: Quantity::ONE,
                tax_class: TaxClass::standard(1500),
            },
            SaleEvent::TenderRecorded {
                sale_id: their_sale,
                tender_id: id(0x702),
                method: TenderMethod::Cash,
                amount: Money::from_minor(32_000, BDT),
                reference: None,
            },
            SaleEvent::Completed {
                sale_id: their_sale,
                total: Money::from_minor(32_000, BDT),
                change_given: Money::from_minor(0, BDT),
                at: at(20),
            },
        ] {
            let envelope = their_events
                .append(
                    EventHeader {
                        event_id: Uuid::now_v7(),
                        tenant_id: identity().tenant_id,
                        outlet_id: identity().outlet_id,
                        device_id: sibling,
                        occurred_at: at(20),
                    },
                    &event,
                )
                .expect("appends");
            seq += 1;
            store.insert_remote(&envelope, seq).expect("stores");
        }

        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(
            reloaded.book().len(),
            2,
            "both tills' sales survive a restart"
        );
        assert_eq!(
            reloaded.takings(BDT).expect("sums"),
            Money::from_minor(43_500, BDT),
            "the outlet's takings, not this till's"
        );
    }

    #[test]
    fn a_siblings_invoices_do_not_advance_this_devices_counter() {
        // Invoices arrive through sync from other tills. Each device owns its own sequence, so a
        // busy neighbour must not push this till's next invoice number forward — two tills would
        // otherwise race each other up the same counter and leave gaps in both.
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        ring_up(&mut till, 0x100, 11_500);
        settle(&mut till, 0x100, 11_500);

        // A sibling device seals three invoices of its own and they reach this store through sync.
        let sibling = id(0x51B);
        let mut their_chain = sahl_core::ledger::FiscalChain::new(sibling);
        let mut their_events = sahl_core::event::EventChain::new(sibling);
        let (mut store, book) = till.into_parts();

        for n in 1..=3_u128 {
            let content = sahl_core::ledger::InvoiceContent {
                totals: sahl_core::tax::calculate(&sahl_core::tax::OrderInput::new(
                    BDT,
                    vec![sahl_core::tax::LineInput::new(
                        Money::from_minor(5_000, BDT),
                        Quantity::ONE,
                        TaxClass::standard(1500),
                    )],
                ))
                .expect("calculates"),
                regime: "bd_mushak".to_owned(),
            };
            let seal = their_chain
                .seal(id(0x900 + n), at(10), &content)
                .expect("seals");
            let envelope = their_events
                .append(
                    EventHeader {
                        event_id: Uuid::now_v7(),
                        tenant_id: identity().tenant_id,
                        outlet_id: identity().outlet_id,
                        device_id: sibling,
                        occurred_at: at(10),
                    },
                    &sahl_core::ledger::FiscalEvent::InvoiceIssued {
                        seal,
                        content,
                        at: at(10),
                        issued_by: id(CASHIER),
                    },
                )
                .expect("appends");
            store
                .insert_remote(&envelope, i64::try_from(n).expect("small"))
                .expect("stores");
        }
        drop(book);

        let reloaded = Terminal::load(store, identity()).expect("reloads");
        assert_eq!(
            reloaded.fiscal_tip().counter,
            1,
            "three of the sibling's invoices, and this till is still on its own first"
        );
    }

    #[test]
    fn adding_the_same_item_twice_could_merge_or_split() {
        // The aggregate allows both — merging is the till's decision, not the domain's, because a
        // cafe with modifiers legitimately wants two rows for two identical drinks.
        let mut till = fresh();
        till.record(&opened(), id(100), at(0)).expect("opens");
        till.record(&line(48_000), id(101), at(1)).expect("adds");

        let existing = till.sale(id(SALE)).expect("sale").active_lines().count();
        assert_eq!(existing, 1);

        // Merging is expressed as a quantity change on the line already there.
        till.record(
            &SaleEvent::LineQuantityChanged {
                sale_id: id(SALE),
                line_id: id(11),
                quantity: Quantity::from_milli(2_000),
            },
            id(102),
            at(2),
        )
        .expect("merges");

        let sale = till.sale(id(SALE)).expect("sale");
        assert_eq!(sale.active_lines().count(), 1, "one row, not two");
        assert_eq!(
            sale.totals().expect("totals").total,
            Money::from_minor(96_000, BDT),
            "and the money doubled"
        );
    }

    #[test]
    fn the_three_nil_treatments_stay_distinct_in_the_summary() {
        // Standard-at-zero, zero-rated and exempt all charge the customer nothing, so no total on
        // any screen would reveal them being collapsed — but they are three different lines on a
        // VAT return, and only exempt blocks reclaiming input VAT.
        let mut till = fresh();
        till.record(&opened(), id(100), at(0)).expect("opens");

        for (line_id, class) in [
            (0x21_u128, TaxClass::standard(0)),
            (0x22, TaxClass::ZeroRated),
            (0x23, TaxClass::Exempt),
        ] {
            till.record(
                &SaleEvent::LineAdded {
                    sale_id: id(SALE),
                    line_id: id(line_id),
                    product_id: id(line_id),
                    name: format!("Item {line_id}"),
                    unit_price: Money::from_minor(9_000, BDT),
                    quantity: Quantity::ONE,
                    tax_class: class,
                },
                id(line_id + 0x1000),
                at(1),
            )
            .expect("adds");
        }

        let totals = till.sale(id(SALE)).expect("sale").totals().expect("totals");

        assert_eq!(totals.tax, Money::from_minor(0, BDT), "nobody was charged");
        assert_eq!(
            totals.tax_groups.len(),
            3,
            "and yet three separate treatments survive to the summary"
        );
        assert!(
            totals
                .tax_groups
                .iter()
                .any(|group| group.tax_class == TaxClass::Exempt)
        );
        assert!(
            totals
                .tax_groups
                .iter()
                .any(|group| group.tax_class == TaxClass::ZeroRated)
        );
    }

    // ------------------------------------------------------------------------------------------
    // Outlet setup
    // ------------------------------------------------------------------------------------------

    fn outlet_settings() -> sahl_core::outlet::OutletSettings {
        sahl_core::outlet::OutletSettings {
            name: "Karim Store".to_owned(),
            profile: sahl_core::outlet::Profile::Retail,
            currency: BDT,
            timezone: "Asia/Dhaka".to_owned(),
            regime: sahl_core::outlet::FiscalRegime::BdMushak,
            tax_registration: Some("0031234567890".to_owned()),
            address: "12 Dhanmondi 27, Dhaka".to_owned(),
        }
    }

    fn configure(settings: sahl_core::outlet::OutletSettings) -> OutletEvent {
        OutletEvent::Configured {
            outlet_id: identity().outlet_id,
            settings,
            at: at(0),
            configured_by: id(0x0E),
        }
    }

    #[test]
    fn an_unconfigured_till_issues_under_no_regime() {
        // It can still sell. A shop trades before anyone finishes setup, and refusing sales until
        // a BIN is typed would make the first morning worse than the paperwork.
        let till = fresh();
        assert!(till.outlet().is_none());
        assert_eq!(till.regime(), "none");
    }

    #[test]
    fn configuring_the_outlet_changes_the_regime_invoices_are_issued_under() {
        let mut till = fresh();
        till.record_outlet(&configure(outlet_settings()), id(50), at(0))
            .expect("configures");

        assert_eq!(till.regime(), "bd_mushak");
        assert_eq!(
            till.outlet()
                .expect("configured")
                .tax_registration
                .as_deref(),
            Some("0031234567890")
        );
    }

    #[test]
    fn a_configuration_that_cannot_trade_is_refused_before_it_is_written() {
        // A Mushak outlet with no BIN would trade all morning and then be unable to issue a single
        // valid challan for the day.
        let mut till = fresh();
        let unsynced_before = till.unsynced_count().expect("counts");

        let refused = till.record_outlet(
            &configure(sahl_core::outlet::OutletSettings {
                tax_registration: None,
                ..outlet_settings()
            }),
            id(50),
            at(0),
        );

        assert!(matches!(refused, Err(TerminalError::Outlet(_))));
        assert!(till.outlet().is_none());
        assert_eq!(till.unsynced_count().expect("counts"), unsynced_before);
    }

    #[test]
    fn the_configuration_survives_a_restart() {
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        till.record_outlet(&configure(outlet_settings()), id(50), at(0))
            .expect("configures");

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(reloaded.regime(), "bd_mushak");
        assert_eq!(reloaded.outlet().expect("configured").name, "Karim Store");
    }

    #[test]
    fn a_later_configuration_replaces_the_earlier_one_whole() {
        // Settings are a replacement, not a patch: a patch arriving out of order would leave an
        // outlet in a state nobody chose.
        let mut till = fresh();
        till.record_outlet(&configure(outlet_settings()), id(50), at(0))
            .expect("configures");
        till.record_outlet(
            &OutletEvent::Configured {
                outlet_id: identity().outlet_id,
                settings: sahl_core::outlet::OutletSettings {
                    profile: sahl_core::outlet::Profile::Cafe,
                    ..outlet_settings()
                },
                at: at(10),
                configured_by: id(0x0E),
            },
            id(51),
            at(10),
        )
        .expect("reconfigures");

        let outlet = till.outlet().expect("configured");
        assert_eq!(outlet.profile, sahl_core::outlet::Profile::Cafe);
        assert!(outlet.can(sahl_core::outlet::Capability::OpenTickets));
    }

    #[test]
    fn a_completed_sale_records_the_configured_regime() {
        let mut till = fresh();
        till.record_outlet(&configure(outlet_settings()), id(50), at(0))
            .expect("configures");
        ring_up(&mut till, 0x100, 11_500);

        till.complete_sale(
            &SaleEvent::Completed {
                sale_id: id(0x100),
                total: Money::from_minor(11_500, BDT),
                change_given: Money::from_minor(0, BDT),
                at: at(3),
            },
            till.regime(),
            id(CASHIER),
            at(3),
        )
        .expect("completes");

        let stored = till.store.load_all().expect("reads");
        let issued = stored
            .iter()
            .find(|envelope| envelope.kind == "fiscal.invoice_issued")
            .expect("present")
            .payload_as::<sahl_core::ledger::FiscalEvent>()
            .expect("decodes");

        let sahl_core::ledger::FiscalEvent::InvoiceIssued { content, .. } = issued;
        assert_eq!(content.regime, "bd_mushak", "not the hardcoded none");
    }

    // ------------------------------------------------------------------------------------------
    // Producing a challan
    // ------------------------------------------------------------------------------------------

    #[test]
    fn an_unconfigured_outlet_owes_no_document() {
        let mut till = fresh();
        ring_up(&mut till, 0x100, 11_500);
        settle(&mut till, 0x100, 11_500);

        assert_eq!(
            till.fiscal_document(id(0x100)).expect("builds"),
            sahl_fiscal::Document::None
        );
    }

    #[test]
    fn a_mushak_outlet_produces_a_challan_with_the_invoice_number() {
        let mut till = fresh();
        till.record_outlet(&configure(outlet_settings()), id(50), at(0))
            .expect("configures");
        ring_up(&mut till, 0x100, 11_500);
        let seal = settle(&mut till, 0x100, 11_500);

        let document = till.fiscal_document(id(0x100)).expect("builds");
        let sahl_fiscal::Document::BdMushak63(challan) = document else {
            panic!("expected a Mushak");
        };

        assert_eq!(challan.invoice_number, seal.counter.to_string());
        assert_eq!(challan.seller_bin, "0031234567890");
        assert_eq!(challan.issuing_address, "12 Dhanmondi 27, Dhaka");
        assert_eq!(challan.lines.len(), 1);
        // The shelf price is tax-inclusive; column 6 must be the net.
        assert_eq!(challan.lines[0].total_value, Money::from_minor(10_000, BDT));
        assert_eq!(challan.lines[0].vat_amount, Money::from_minor(1_500, BDT));
        assert_eq!(challan.total_with_tax, Money::from_minor(11_500, BDT));
    }

    #[test]
    fn a_sale_that_was_never_completed_has_no_document() {
        // The invoice number comes from completion. Asking before then is a question with no
        // answer, not a document with a blank number.
        let mut till = fresh();
        till.record_outlet(&configure(outlet_settings()), id(50), at(0))
            .expect("configures");
        ring_up(&mut till, 0x100, 11_500);

        assert!(matches!(
            till.fiscal_document(id(0x100)),
            Err(TerminalError::NotInvoiced { .. })
        ));
    }

    #[test]
    fn a_large_sale_is_refused_a_challan_until_the_buyer_is_named() {
        // Rule 40(1): above Tk 25,000 the buyer must be named with address and BIN. Nothing at the
        // counter captures that yet, so the document layer refuses — which is the correct failure.
        // The sale itself still completed, because a till that refuses to sell is worse.
        let mut till = fresh();
        till.record_outlet(&configure(outlet_settings()), id(50), at(0))
            .expect("configures");
        ring_up(&mut till, 0x100, 3_000_000);
        settle(&mut till, 0x100, 3_000_000);

        assert!(
            till.sale(id(0x100)).expect("sale").settled_at().is_some(),
            "the sale went through"
        );
        assert!(
            matches!(
                till.fiscal_document(id(0x100)),
                Err(TerminalError::FiscalDocument(_))
            ),
            "but the challan cannot be issued blank"
        );
    }

    #[test]
    fn the_challan_survives_a_restart() {
        // It is rebuilt from the log rather than stored, so this is really asking whether the seal
        // and the outlet config both came back.
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        till.record_outlet(&configure(outlet_settings()), id(50), at(0))
            .expect("configures");
        ring_up(&mut till, 0x100, 11_500);
        settle(&mut till, 0x100, 11_500);

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        let sahl_fiscal::Document::BdMushak63(challan) =
            reloaded.fiscal_document(id(0x100)).expect("builds")
        else {
            panic!("expected a Mushak");
        };
        assert_eq!(challan.invoice_number, "1");
    }

    // ------------------------------------------------------------------------------------------
    // The catalogue
    // ------------------------------------------------------------------------------------------

    fn product_details(
        name: &str,
        minor: i64,
        unit: sahl_core::catalogue::Unit,
    ) -> sahl_core::catalogue::ProductDetails {
        sahl_core::catalogue::ProductDetails {
            name: name.to_owned(),
            sku: None,
            barcodes: vec!["8901".to_owned()],
            price: Money::from_minor(minor, BDT),
            unit,
            tax_class: TaxClass::standard(1500),
            category: Some("Staples".to_owned()),
        }
    }

    #[test]
    fn a_product_survives_a_restart() {
        let store = EventStore::open_in_memory(id(3)).expect("opens");
        let mut till = Terminal::load(store, identity()).expect("loads");
        till.record_catalogue(
            &sahl_core::catalogue::CatalogueEvent::ProductAdded {
                product_id: id(0x101),
                details: product_details(
                    "Rice, loose",
                    4_600,
                    sahl_core::catalogue::Unit::Kilogram,
                ),
                at: at(0),
                added_by: id(0x0E),
            },
            id(60),
            at(0),
        )
        .expect("adds");

        let (store, _) = till.into_parts();
        let reloaded = Terminal::load(store, identity()).expect("reloads");

        assert_eq!(reloaded.catalogue().sellable().len(), 1);
        assert_eq!(
            reloaded.catalogue().by_barcode("8901").expect("found").name,
            "Rice, loose"
        );
    }

    #[test]
    fn the_challan_takes_its_unit_of_supply_from_the_catalogue() {
        // The reason the catalogue had to exist before a challan could be right: every line printed
        // "pcs" regardless, and Unit of Supply is a column on the form.
        let mut till = fresh();
        till.record_outlet(&configure(outlet_settings()), id(50), at(0))
            .expect("configures");
        till.record_catalogue(
            &sahl_core::catalogue::CatalogueEvent::ProductAdded {
                product_id: id(12),
                details: product_details(
                    "Rice, loose",
                    4_600,
                    sahl_core::catalogue::Unit::Kilogram,
                ),
                at: at(0),
                added_by: id(0x0E),
            },
            id(60),
            at(0),
        )
        .expect("adds");

        // `ring_up` uses product id 12, matching the catalogue entry above.
        ring_up(&mut till, 0x100, 11_500);
        settle(&mut till, 0x100, 11_500);

        let sahl_fiscal::Document::BdMushak63(challan) =
            till.fiscal_document(id(0x100)).expect("builds")
        else {
            panic!("expected a Mushak");
        };
        assert_eq!(challan.lines[0].unit, "kg", "not the hardcoded pcs");
    }

    #[test]
    fn a_line_for_a_product_this_device_has_never_seen_still_prints() {
        // A sibling's sale can arrive before its catalogue entry. Falling back to pieces is wrong
        // for a weighed good, but refusing to print the challan at all would be worse.
        let mut till = fresh();
        till.record_outlet(&configure(outlet_settings()), id(50), at(0))
            .expect("configures");
        ring_up(&mut till, 0x100, 11_500);
        settle(&mut till, 0x100, 11_500);

        let sahl_fiscal::Document::BdMushak63(challan) =
            till.fiscal_document(id(0x100)).expect("builds")
        else {
            panic!("expected a Mushak");
        };
        assert_eq!(challan.lines[0].unit, "pcs");
    }

    #[test]
    fn a_catalogue_edit_does_not_rewrite_what_a_customer_already_paid() {
        // The whole reason last-writer-wins is safe for catalogue edits: the sale line snapshots
        // its price, so a later price rise cannot change a settled sale.
        let mut till = fresh();
        till.record_catalogue(
            &sahl_core::catalogue::CatalogueEvent::ProductAdded {
                product_id: id(12),
                details: product_details("Rice 5kg", 48_000, sahl_core::catalogue::Unit::Piece),
                at: at(0),
                added_by: id(0x0E),
            },
            id(60),
            at(0),
        )
        .expect("adds");

        ring_up(&mut till, 0x100, 48_000);
        settle(&mut till, 0x100, 48_000);

        till.record_catalogue(
            &sahl_core::catalogue::CatalogueEvent::ProductUpdated {
                product_id: id(12),
                details: product_details("Rice 5kg", 52_000, sahl_core::catalogue::Unit::Piece),
                at: at(20),
                updated_by: id(0x0E),
            },
            id(61),
            at(20),
        )
        .expect("updates");

        assert_eq!(
            till.sale(id(0x100))
                .expect("sale")
                .totals()
                .expect("totals")
                .total,
            Money::from_minor(48_000, BDT),
            "the settled sale is untouched"
        );
        assert_eq!(
            till.catalogue().get(id(12)).expect("present").price,
            Money::from_minor(52_000, BDT),
            "while the catalogue moved on"
        );
    }
}
