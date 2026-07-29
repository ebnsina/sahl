//! # sahl-server
//!
//! Sync ingest, API, and background jobs. Compiles `sahl-core` in, so every total it computes is
//! produced by the same code the terminal runs — the reason the stack is split this way at all.

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

pub mod config;
pub mod db;
pub mod device;
pub mod sync;

pub use config::{Config, ConfigError};
