//! What the event log says about how the till is being used.
//!
//! This is nearly free given the log: the same append-only record that makes offline selling
//! correct is a complete account of who did what, and detection is arithmetic over it.
//!
//! ## A finding is a question, not an accusation
//!
//! Every signal here has an innocent explanation. The cashier with twice everyone's void rate may
//! be the one on the returns counter. The discounts stopping just under the approval limit may be
//! a manager's standing instruction nobody wrote down. Someone approving their own void may be the
//! owner, alone, at seven in the morning.
//!
//! So nothing here concludes anything. Each finding names what was counted and who it concerns,
//! and the owner — who knows which of their staff works the returns counter — decides what it
//! means. Wording that reads as an accusation would make this feature actively harmful the first
//! time it was wrong about somebody, and it will be wrong about somebody.
//!
//! ## What it deliberately does not do
//!
//! No thresholds are invented. Every comparison is against the outlet's own behaviour or against a
//! limit the owner already set in the approval policy. A rule like "more than five voids is
//! suspicious" would be a number nobody chose, applied to shops nobody has seen.

mod finding;
mod scan;

pub use finding::{Finding, Subject};
pub use scan::{Activity, Sensitivity, scan};
