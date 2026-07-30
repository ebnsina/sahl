//! What a day came to.
//!
//! Computed from completed sales, in the same crate the till computes them with. The owner
//! dashboard and the terminal must not reach different totals for the same day — and they would,
//! eventually, if the dashboard added its own arithmetic in TypeScript over numbers it fetched.
//!
//! Everything here is derived. Nothing is stored, so nothing can disagree with the log.

mod day;

pub use day::{CashierRow, Day, PaymentRow, ProductRow};
