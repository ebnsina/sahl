//! Conflict rules for two tills that were apart.
//!
//! Each rule is stated where it can be tested without a database or a network, because these are
//! the decisions that are hardest to reason about and easiest to get subtly wrong.

pub mod catalogue;
pub mod lease;
pub mod stock;

pub use catalogue::{CatalogueEdit, latest_per_product, resolve};
pub use lease::{
    ClaimVerdict, LEASE_IDLE_TIMEOUT_MILLIS, TicketLease, evaluate_claim, resolve_contest,
};
pub use stock::{
    MovementReason, Oversell, StockMovement, StockVerdict, check, detect_oversells, level_of,
};
