//! The sale aggregate — the write model at the heart of the till.
//!
//! A sale is **events, not a row**. [`Sale`] is what you get by replaying them, never something
//! stored and mutated. That is what makes the day auditable, what the fraud detection reads, and
//! what lets a terminal keep selling with no network and reconcile later.
//!
//! The vocabulary is deliberately ticket-shaped rather than receipt-shaped: a sale is opened, lives
//! for a while, and is closed. Retail is the degenerate case where that lifetime is a few seconds; a
//! café ticket sits open for an hour. Building retail-first and adding tables later is exactly how
//! one codebase becomes two POS products, so the model is restaurant-grade from the start.
//!
//! Two rules run through everything here:
//!
//! - **Lines snapshot their price, name and tax class at the moment of sale.** A price change from
//!   the back office must never alter a receipt already printed, and both fiscal regimes assume a
//!   reprint matches the original.
//! - **Voided lines are kept and flagged, never deleted.** A cashier who rings a sale, takes the
//!   cash, then voids the line leaves no other trace — so the trace is not optional.

mod error;
mod event;
mod line;
mod split;
mod state;
mod tender;

pub use error::SaleError;
pub use event::SaleEvent;
pub use line::{LineVoid, Modifier, SaleLine, VoidReason};
pub use split::{SplitError, SplitPart, by_lines, evenly};
pub use state::{Sale, SaleStatus, Seating, cash};
pub use tender::{Tender, TenderMethod, Wallet};
