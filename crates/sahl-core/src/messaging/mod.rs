//! Telling somebody something.
//!
//! ## The channel is a setting, not an assumption
//!
//! WhatsApp dominates the Gulf. In Bangladesh, Messenger and SMS have materially wider reach, and a
//! product that hard-wired WhatsApp would launch into its first market unable to reach half its
//! customers. So the channel is chosen per tenant and everything here is written without knowing
//! which one it is.
//!
//! **This has not been validated with real merchants yet.** The abstraction makes it cheap to be
//! wrong, but only if somebody actually asks — and nobody has.
//!
//! ## Composing a message is not sending one
//!
//! Everything in this module is pure: it decides *what* to say. Sending needs a socket, a provider
//! API and credentials, and belongs to whatever holds those. That split is what lets the wording
//! of a daily digest be tested without a network, and what stops a template change requiring a
//! deployment of anything that talks to Meta.

mod channel;
mod digest;

pub use channel::{Audience, Channel, Message, MessageKind, Recipient};
pub use digest::{closing_summary, low_stock};
