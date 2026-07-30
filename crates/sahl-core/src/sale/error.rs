use thiserror::Error;
use uuid::Uuid;

use crate::money::{Money, MoneyError};
use crate::tax::TaxError;

/// Why a sale rejected an event.
///
/// Every variant is a state the terminal must never reach. Reaching one during replay means the log
/// is inconsistent — a stronger signal than a failed command, since a valid log replays cleanly by
/// construction.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SaleError {
    #[error("money error in sale: {0}")]
    Money(#[from] MoneyError),

    /// Covers is the denominator of every per-head figure a café reports on, so zero is a division
    /// by zero waiting to happen rather than an empty table.
    #[error("a seated ticket needs at least one cover")]
    NoCovers,

    #[error("tax error in sale: {0}")]
    Tax(#[from] TaxError),

    #[error("the first event of a sale must be `opened`, found `{found}`")]
    NotOpenedFirst { found: &'static str },

    #[error("sale was already opened")]
    AlreadyOpened,

    #[error("event belongs to sale {found} but this is sale {expected}")]
    WrongSale { expected: Uuid, found: Uuid },

    #[error("sale is {status} and can no longer be modified")]
    NotOpen { status: &'static str },

    #[error("line {line_id} is already on this sale")]
    DuplicateLine { line_id: Uuid },

    #[error("no line {line_id} on this sale")]
    UnknownLine { line_id: Uuid },

    #[error("line {line_id} is already voided")]
    AlreadyVoided { line_id: Uuid },

    #[error("a sale needs at least one un-voided line")]
    NoActiveLines,

    #[error("cannot complete: {outstanding} is still outstanding")]
    Outstanding { outstanding: Money },

    /// A card or wallet was charged more than the sale came to. Only cash may over-tender, because
    /// only cash can be handed back — giving change against a card over-charge takes real money out
    /// of the drawer for a payment that never arrived.
    #[error("non-cash tender of {tendered} exceeds the total of {total}")]
    NonCashOvertender { tendered: Money, total: Money },

    #[error("recorded change of {recorded} does not match the calculated {calculated}")]
    ChangeMismatch { recorded: Money, calculated: Money },

    #[error("recorded total of {recorded} does not match the calculated {calculated}")]
    TotalMismatch { recorded: Money, calculated: Money },

    #[error("a tender of {amount} is not a positive amount")]
    NonPositiveTender { amount: Money },
}
