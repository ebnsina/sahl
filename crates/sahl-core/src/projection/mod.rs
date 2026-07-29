//! Read models, derived by replaying the event log.
//!
//! Projections are **rebuildable and never authoritative**. The log is the truth; everything here
//! is a cache of it that can be thrown away and recomputed. That is what makes "replay the day and
//! prove the numbers" a real capability rather than a slogan, and it is what lets a terminal
//! recover its whole state from disk after a crash mid-shift.
//!
//! ## Determinism is the whole contract
//!
//! The terminal and the server both replay the same events and must reach byte-identical results.
//! If they can diverge, a merchant has two versions of their day and no way to tell which is right.
//!
//! Two decisions protect that, and both are easy to undo by accident:
//!
//! - **`BTreeMap`, never `HashMap`.** Hash iteration order varies between processes and across
//!   runs, so any output derived by iterating a `HashMap` — a report, a sync payload, a fingerprint
//!   — would differ between two machines holding identical data.
//! - **No clock, no randomness.** Nothing here reads the current time or generates an id. Every
//!   value comes from the events themselves.

mod book;

pub use book::SaleBook;
