//! What an outlet is: its vertical profile and the regime it trades under.
//!
//! Both arrive through the event log like everything else, so a till that has been offline since
//! before a merchant switched profile learns about it through the same push-pull it already runs.

mod config;
mod event;
mod profile;

pub use config::{FiscalRegime, OutletConfig, OutletError};
pub use event::{OutletEvent, OutletSettings};
pub use profile::{Capability, Profile};
