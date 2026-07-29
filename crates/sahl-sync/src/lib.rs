//! # sahl-sync
//!
//! The wire protocol and batch-planning logic, shared by terminal and server so both agree on what
//! a valid push is. Pure — no I/O, no database.

#![forbid(unsafe_code)]
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

mod error;
mod plan;
mod protocol;

pub use error::SyncError;
pub use plan::{BatchPlan, plan_batch};
pub use protocol::{
    MAX_BATCH, PullRequest, PullResponse, PushRequest, PushResponse, SyncCursor, SyncRejection,
};
