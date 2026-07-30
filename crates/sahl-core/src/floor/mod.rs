//! The floor: tables, and where a ticket is sitting.
//!
//! Only the café profile uses this — `Capability::TableService`. It is here rather than behind a
//! feature flag because the profile is a row, not a branch: a retail outlet simply has no tables,
//! and the same binary serves both.
//!
//! Tables are furniture. Which ticket is on one is derived from the open sales rather than stored on
//! the table, because a table holding its own ticket id has to be kept in step with the sale — and
//! the two disagreeing is how a café ends up unable to seat a table it can see is empty.

mod event;
mod plan;
mod table;

pub use event::{FloorEvent, TableDetails};
pub use plan::Floor;
pub use table::{FloorError, MAX_SEATS, Table};
