use thiserror::Error;

use super::currency::Currency;

/// Every way a money operation can refuse to produce a wrong answer.
///
/// Note what is *absent*: there is no variant meaning "close enough". Operations either produce an
/// exact result or fail loudly. A POS that silently rounds its way out of trouble is a POS whose
/// end-of-day till never balances.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MoneyError {
    /// Attempted to combine two different currencies. Always a programming error.
    #[error("currency mismatch: {left} and {right} cannot be combined")]
    CurrencyMismatch { left: Currency, right: Currency },

    /// The result did not fit in `i64` minor units.
    #[error("arithmetic overflow in money operation")]
    Overflow,

    /// A ratio was applied with a zero denominator.
    #[error("division by zero in money operation")]
    DivisionByZero,

    /// Allocation was asked to split across no parts, or across weights summing to zero.
    #[error("allocation requires at least one part with a positive total weight")]
    InvalidWeights,

    /// A currency code outside the set Sahl transacts in.
    #[error("unknown currency code: {0}")]
    UnknownCurrency(String),
}
