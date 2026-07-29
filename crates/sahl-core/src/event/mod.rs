//! The append-only event log: sealed events, hash chains, and verification.
//!
//! This module is the spine of the product, and it earns its keep three times over:
//!
//! - **Offline selling** needs a log a terminal can append to with no network and reconcile later.
//! - **Fraud detection** needs a record that cannot be quietly edited — the owner-facing wedge is
//!   only credible if "the log says so" actually means something.
//! - **Fiscal compliance** needs exactly this shape: ZATCA mandates a SHA-256 chain in which each
//!   invoice embeds its predecessor's digest.
//!
//! One mechanism satisfies all three, which is why it is built on day one rather than retrofitted.
//! Adding a hash chain to a financial ledger that is already live in shops is the single most
//! painful migration this product could face.
//!
//! Read models — products, stock, open tickets, shifts — are **projections** of this log, never the
//! source of truth. That is what makes "replay the day and prove the numbers" possible.
//!
//! Nothing here reads a clock or generates an ID. Callers supply both, which keeps sealing a pure
//! function and therefore identical on the terminal and on the server.

mod canonical;
mod chain;
mod envelope;
mod error;
mod hash;

pub use canonical::canonical_bytes;
pub use chain::{ChainTip, EventChain, verify_chain, verify_chain_from_genesis};
pub use envelope::{EventEnvelope, EventHeader, EventPayload};
pub use error::EventError;
pub use hash::EventHash;
