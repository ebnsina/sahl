//! What a shop sells.
//!
//! Products arrive through the event log like everything else, so a till offline since Tuesday
//! learns about Wednesday's price rise through the same push-pull it already runs.
//!
//! Catalogue edits are the one class the plan resolves by last-writer-wins, and that is only safe
//! because of a decision made elsewhere: **every sale line snapshots the price it charged.** A price
//! that changes on two devices at once resolves to one of them, and neither outcome can rewrite what
//! a customer already paid.

mod book;
mod event;
mod import;
mod options;
mod product;

pub use book::Catalogue;
pub use event::{CatalogueEvent, ProductDetails};
pub use import::{Import, ImportError, ImportProblem, ImportedProduct, from_delimited};
pub use options::{ModifierGroup, ModifierOption};
pub use product::{CatalogueError, Product, Unit};
