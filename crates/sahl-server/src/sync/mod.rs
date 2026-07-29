//! Sync ingest and pull.
//!
//! Everything runs inside one transaction per request, so a batch is all-or-nothing: a
//! half-committed batch leaves a chain the terminal cannot resume from.

mod error;
mod ingest;

pub use error::IngestError;
pub use ingest::{DeviceRecord, load_device, pull, push};
