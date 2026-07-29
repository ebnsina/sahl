//! # sahl-core
//!
//! The pure domain core of Sahl. This crate is compiled into **both** the Tauri terminal and the
//! Axum server, which is the whole point: the register and the cloud run the same money code, so
//! they cannot disagree about a total.
//!
//! Two constraints keep that property true, and both are enforced rather than documented:
//!
//! - **No I/O, no async.** No `tokio`, no `sqlx`, no filesystem. Everything here is a pure function
//!   over values, which is what makes it portable to a WASM webview, an Android binary, and a
//!   server process without change.
//! - **No floating point.** `clippy::float_arithmetic` is `deny` at the workspace root. Money is
//!   integer minor units, always.

#![forbid(unsafe_code)]
// The workspace denies `unwrap`, `expect`, `panic` and unchecked arithmetic, because in production
// code each of those is a way for a wrong number to reach a till. Tests are the exact inverse: a
// test asserts *by* panicking, and forcing them through `Result` would obscure what they prove.
// Scoped to `cfg(test)` so the production rules stay absolute.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
    )
)]

pub mod event;
pub mod inventory;
pub mod money;
pub mod policy;
pub mod projection;
pub mod quantity;
pub mod sale;
pub mod shift;
pub mod staff;
pub mod tax;
pub mod time;

pub use event::{
    ChainTip, EventChain, EventEnvelope, EventError, EventHash, EventHeader, EventPayload,
    verify_chain, verify_chain_from_genesis,
};
pub use money::{Currency, Money, MoneyError, Rate, Rounding};
pub use projection::SaleBook;
pub use quantity::Quantity;
pub use sale::{Sale, SaleError, SaleEvent, SaleLine, SaleStatus, TenderMethod, VoidReason};
pub use tax::{
    Discount, LineInput, LineTotals, OrderInput, OrderTotals, PricingMode, TaxClass, TaxError,
    TaxGroup, calculate,
};
pub use time::Timestamp;
