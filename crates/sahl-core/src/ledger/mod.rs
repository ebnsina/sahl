//! The fiscal ledger: invoice counters and the invoice hash chain.
//!
//! Deliberately separate from the event log. The event chain proves nothing was altered in the
//! *record of what happened*; this chain proves nothing was altered in the *sequence of invoices* —
//! and a regime cares about the second even where it has never heard of the first. An invoice can
//! be voided and superseded in the event log while its place in the fiscal sequence stays fixed
//! forever, which is exactly what a tax authority means by an audit trail.
//!
//! The shape is ZATCA's, because ZATCA's is the strictest of the two target markets: a per-device
//! monotonic counter (the ICV) and each invoice embedding its predecessor's hash (the PIH). Mushak
//! 6.3 needs only the counter, and takes it from here rather than keeping its own — two counters
//! for one sequence is two things that can disagree.
//!
//! Building this on day one is the whole point. Retrofitting a hash chain onto a live financial
//! ledger is the single most painful migration in a product like this; doing it now costs nothing
//! because the invoices do not exist yet.

mod chain;
mod counter;
mod event;

pub use chain::{FiscalChain, FiscalError, FiscalTip, InvoiceSeal, verify_invoice_chain};
pub use counter::InvoiceCounter;
pub use event::{FiscalEvent, InvoiceContent};
