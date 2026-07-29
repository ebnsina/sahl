//! The VAT engine.
//!
//! Turns an [`OrderInput`] into [`OrderTotals`]: per-line net/tax/total, a VAT summary grouped by
//! tax class, and order aggregates that are the exact sum of their lines.
//!
//! Three decisions here are worth knowing before reading the code, because each is a place where
//! the obvious implementation is subtly wrong:
//!
//! - **Tax-inclusive is the default.** Both target markets price at retail inclusive of VAT — a
//!   Bangladeshi MRP printed on the packet, a Gulf B2C shelf label. Inclusive mode computes tax
//!   first and subtracts, so `net + tax` reconstructs the label price exactly.
//! - **Order discounts are apportioned back across lines**, not subtracted from the total. On a
//!   mixed-rate basket, subtracting at the end computes VAT on an undiscounted base and overstates
//!   the tax owed.
//! - **Zero-rated and exempt are distinct classes**, not both "rate = 0". The arithmetic matches;
//!   the filing does not.

mod class;
mod discount;
mod engine;
mod error;
mod order;
mod totals;

pub use class::TaxClass;
pub use discount::Discount;
pub use engine::calculate;
pub use error::TaxError;
pub use order::{LineInput, OrderInput, PricingMode};
pub use totals::{LineTotals, OrderTotals, TaxGroup};
