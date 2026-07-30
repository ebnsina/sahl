//! Selling things that are weighed.
//!
//! Two ways a weight reaches the till, and they are not interchangeable.
//!
//! A **label** is printed at the deli counter by a scale that has already done the arithmetic; the
//! cashier scans it. The weight — or worse, the price — is buried in the digits of an ordinary
//! EAN-13. Nothing about the barcode announces this, which is why the format is configured per
//! outlet and never inferred.
//!
//! A **live reading** comes off a scale on the counter while the customer waits. That one is a
//! hardware concern and lives in the terminal; what belongs here is deciding whether a reading may
//! become a sale line at all.

mod barcode;
mod weighed;

pub use barcode::{Embedded, ScaleFormat, ScaleScan, ScannedValue};
pub use weighed::{WeighError, weigh};

/// Anything that can go wrong reading a scale label.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScaleError {
    #[error("a scale label is 13 digits, this one is {length}")]
    WrongLength { length: usize },

    #[error("a barcode is digits only, found {found:?}")]
    NotANumber { found: char },

    #[error("{barcode} does not start with the scale prefix {prefix}")]
    NotAScaleLabel { barcode: String, prefix: String },

    #[error("check digit is {found}, expected {expected} — the scan is corrupt")]
    BadCheckDigit { found: u32, expected: u32 },

    #[error("{prefix}+{item}+{value}+{filler}+1 check digit is {total} digits, not 13")]
    BadFormat {
        prefix: usize,
        item: u8,
        value: u8,
        filler: u8,
        total: usize,
    },

    #[error("a prefix is 1 to 3 digits, got {length}")]
    BadPrefix { length: usize },

    #[error("{decimals} decimal places cannot be held in {unit}")]
    TooPrecise { decimals: u8, unit: &'static str },

    #[error("arithmetic error: {0}")]
    Money(#[from] crate::money::MoneyError),
}
