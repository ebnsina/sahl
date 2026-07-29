use thiserror::Error;

use crate::money::{Currency, MoneyError};

/// Why a tax calculation could not produce an exact answer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TaxError {
    /// An underlying money operation failed.
    #[error("money error during tax calculation: {0}")]
    Money(#[from] MoneyError),

    /// A line was denominated in a different currency from the order.
    #[error("line {index} is in {found} but the order is in {expected}")]
    LineCurrencyMismatch {
        index: usize,
        expected: Currency,
        found: Currency,
    },

    /// An order with no lines was submitted for calculation.
    #[error("cannot calculate tax for an order with no lines")]
    EmptyOrder,
}
