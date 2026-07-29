//! Batches, expiry, and picking stock.
//!
//! Batches are identity rather than metadata: two crates of the same product with different expiry
//! dates are different things. That is what lets a pharmacy answer "which customers received lot X"
//! after a recall, and a grocery sell down what expires first.

mod batch;
mod ledger;

pub use batch::Batch;
pub use ledger::{
    Allocation, BatchLevel, Pick, by_product, expired, expiring_soon, pick_fefo, sellable_on_hand,
    total_on_hand,
};
