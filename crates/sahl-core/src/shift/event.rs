use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventPayload;
use crate::money::{Currency, Money};
use crate::time::Timestamp;

/// Why cash moved in or out of the drawer outside a sale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CashMovementReason {
    /// Change brought from the safe mid-shift.
    FloatTopUp,
    /// Cash lifted to the safe, so a busy till does not hold a day's takings.
    Skim,
    /// Paid a supplier or a courier from the drawer — ordinary in these markets.
    PettyCash,
    /// Refund handed over outside a sale.
    Refund,
    /// Counted short or over and corrected. Always suspicious, always recorded.
    Correction,
}

/// Everything that can happen to a till session.
///
/// The kind strings are hashed into the chain, so they are a wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShiftEvent {
    /// A cashier takes the till, counting in the starting float.
    Opened {
        shift_id: Uuid,
        opened_by: Uuid,
        currency: Currency,
        opening_float: Money,
        at: Timestamp,
    },

    /// Cash moved outside a sale.
    CashMoved {
        shift_id: Uuid,
        movement_id: Uuid,
        /// Positive in, negative out.
        amount: Money,
        reason: CashMovementReason,
        note: Option<String>,
        authorized_by: Uuid,
        at: Timestamp,
    },

    /// A physical count of the drawer.
    ///
    /// Recorded **before** the cashier is shown what was expected — see [`super::Shift`]. The
    /// counted figure is what they found, not what they were told to find.
    Counted {
        shift_id: Uuid,
        counted: Money,
        counted_by: Uuid,
        at: Timestamp,
    },

    /// The session ends. Nothing may be added afterwards.
    Closed {
        shift_id: Uuid,
        closed_by: Uuid,
        /// What the drawer actually held, from the final count.
        closing_cash: Money,
        at: Timestamp,
    },
}

impl ShiftEvent {
    #[must_use]
    pub const fn shift_id(&self) -> Uuid {
        match self {
            Self::Opened { shift_id, .. }
            | Self::CashMoved { shift_id, .. }
            | Self::Counted { shift_id, .. }
            | Self::Closed { shift_id, .. } => *shift_id,
        }
    }

    #[must_use]
    pub const fn at(&self) -> Timestamp {
        match self {
            Self::Opened { at, .. }
            | Self::CashMoved { at, .. }
            | Self::Counted { at, .. }
            | Self::Closed { at, .. } => *at,
        }
    }
}

impl EventPayload for ShiftEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::Opened { .. } => "shift.opened",
            Self::CashMoved { .. } => "shift.cash_moved",
            Self::Counted { .. } => "shift.counted",
            Self::Closed { .. } => "shift.closed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_strings_are_stable_and_namespaced() {
        // Hashed into the chain; a rename invalidates every shift already recorded in the field.
        let opened = ShiftEvent::Opened {
            shift_id: Uuid::from_u128(1),
            opened_by: Uuid::from_u128(2),
            currency: Currency::Bdt,
            opening_float: Money::from_minor(500_000, Currency::Bdt),
            at: Timestamp::from_millis(0),
        };
        assert_eq!(opened.kind(), "shift.opened");
        assert_eq!(opened.shift_id(), Uuid::from_u128(1));
    }

    #[test]
    fn events_round_trip_through_json() {
        let event = ShiftEvent::CashMoved {
            shift_id: Uuid::from_u128(1),
            movement_id: Uuid::from_u128(3),
            amount: Money::from_minor(-20_000, Currency::Bdt),
            reason: CashMovementReason::Skim,
            note: Some("to safe".to_owned()),
            authorized_by: Uuid::from_u128(4),
            at: Timestamp::from_millis(5),
        };
        let encoded = serde_json::to_string(&event).expect("serialises");
        assert!(encoded.contains(r#""type":"cash_moved""#));
        assert_eq!(
            serde_json::from_str::<ShiftEvent>(&encoded).expect("deserialises"),
            event
        );
    }
}
