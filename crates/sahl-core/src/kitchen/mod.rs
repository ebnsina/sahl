//! Kitchen order tickets.
//!
//! The hard part is not printing — it is remembering what a station has already been told. A "send
//! to kitchen" that reprints the whole order gets the food made twice, and unlike almost every other
//! POS mistake that one cannot be corrected after the fact: the second dish is already cooked.
//!
//! So firing is recorded on the sale, not merely performed. Everything here reads that record and
//! works out the difference.

mod station;
mod ticket;

pub use station::Station;
pub use ticket::{KitchenTicket, TicketKind, TicketLine, cancellations, pending};
