//! Purchase orders and stock transfers.
//!
//! Both are documents that *expect* stock to move, sitting one layer above the batch ledger, which
//! only knows that it did. That gap is the point: an order that was never fully delivered and a
//! transfer where nine of ten crates arrived are both invisible to a book that records only what
//! turned up.

mod event;
mod order;
mod transfer;

pub use event::{CloseReason, DispatchLine, OrderLine, PurchaseEvent, TransferEvent};
pub use order::{LineProgress, OrderStatus, PurchaseError, PurchaseOrder};
pub use transfer::{LineTransit, Transfer, TransferError, TransferStatus};
