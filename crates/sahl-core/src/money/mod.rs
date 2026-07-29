//! Money primitives: integer minor units, checked arithmetic, and cent-preserving allocation.
//!
//! Everything downstream — the VAT engine, invoice totals, shift reconciliation — is built on
//! [`Money`]. The invariants it guarantees are the ones the rest of the product assumes without
//! rechecking, so they are tested exhaustively rather than by example.

mod amount;
mod currency;
mod error;
mod rate;
mod rounding;

pub use amount::Money;
pub use currency::Currency;
pub use error::MoneyError;
pub use rate::Rate;
pub use rounding::Rounding;
