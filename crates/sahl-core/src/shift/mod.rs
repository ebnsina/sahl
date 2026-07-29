//! Till sessions and cash reconciliation.
//!
//! A shift is what a drawer count is measured against, which makes it the backbone of the
//! owner-facing fraud signals: variance per cashier, recounts, and cash lifted mid-shift are only
//! meaningful relative to a session.

mod event;
mod report;
mod state;

pub use event::{CashMovementReason, ShiftEvent};
pub use report::{ShiftReport, Variance, report};
pub use state::{CashMovement, DrawerCount, Shift, ShiftError, ShiftStatus};
