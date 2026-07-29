//! The sync client.
//!
//! Push first, then pull: a shop's own sales exist nowhere else, while a sibling's are already safe
//! on the server. If a flaky connection cuts a round short, the half that ran is the half that
//! reduces risk.

mod backoff;
mod engine;
mod http;
mod scheduler;

pub use backoff::Backoff;
pub use engine::{SyncClientError, SyncOutcome, Transport, sync_once};
pub use http::HttpTransport;
pub use scheduler::{SyncHandle, SyncStatus, spawn};
