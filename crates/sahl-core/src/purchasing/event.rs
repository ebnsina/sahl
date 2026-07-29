//! Purchase and transfer events.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventPayload;
use crate::money::Money;
use crate::quantity::Quantity;
use crate::time::Timestamp;

/// A line on an order or a dispatch: what, how much, at what cost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderLine {
    pub line_id: Uuid,
    pub product_id: Uuid,
    pub quantity: Quantity,
    /// What we expect to pay per unit. Recorded on the order so the receipt can disagree with it,
    /// which is the whole reason to keep an order at all — a supplier who quietly raises a price
    /// between quote and delivery is invisible without a document to compare against.
    pub unit_cost: Money,
}

/// Why an order stopped short of being fully received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CloseReason {
    /// Everything ordered arrived.
    Complete,
    /// The rest is never coming; stop expecting it.
    ShortShipped,
    /// Called off before delivery.
    Cancelled,
}

/// Everything that happens to a purchase order.
///
/// Kind strings are hashed into the chain, so they are a wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PurchaseEvent {
    /// An order was placed with a supplier.
    Placed {
        order_id: Uuid,
        supplier: String,
        /// Reference the supplier knows it by — their invoice or PO number.
        reference: Option<String>,
        lines: Vec<OrderLine>,
        expected_at: Option<Timestamp>,
        at: Timestamp,
        placed_by: Uuid,
    },

    /// Some or all of a line arrived and became a batch.
    ///
    /// Separate from the order, and repeatable per line, because part deliveries are the norm and a
    /// receipt that could only be recorded in full would push staff into recording it as one lump
    /// on the day the last box turns up.
    LineReceived {
        order_id: Uuid,
        line_id: Uuid,
        /// The batch this became — the join between the document and the shelf.
        batch_id: Uuid,
        quantity: Quantity,
        /// What was actually charged, which may not be what was ordered at.
        unit_cost: Money,
        at: Timestamp,
        received_by: Uuid,
    },

    /// The order is finished with, whether or not everything came.
    Closed {
        order_id: Uuid,
        reason: CloseReason,
        at: Timestamp,
        closed_by: Uuid,
    },
}

/// Everything that happens to a stock transfer between outlets.
///
/// Two events, not one move. The sending and receiving outlets are different devices that may be
/// offline from each other for hours, so stock is genuinely in neither place for a while — and the
/// gap between what was sent and what arrived is exactly what a two-outlet owner wants to see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransferEvent {
    /// Stock left the sending outlet.
    Dispatched {
        transfer_id: Uuid,
        from_outlet: Uuid,
        to_outlet: Uuid,
        lines: Vec<DispatchLine>,
        at: Timestamp,
        dispatched_by: Uuid,
    },

    /// Stock arrived at the receiving outlet and became batches there.
    Received {
        transfer_id: Uuid,
        line_id: Uuid,
        /// The batch created at the destination. A new id: it is a different outlet's stock now,
        /// and reusing the sender's would make the same batch appear to be in two places.
        batch_id: Uuid,
        quantity: Quantity,
        at: Timestamp,
        received_by: Uuid,
    },

    /// The transfer is settled, whatever the discrepancy was.
    Settled {
        transfer_id: Uuid,
        at: Timestamp,
        settled_by: Uuid,
    },
}

/// One line of a dispatch, tied to the source batch so lot traceability survives the move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchLine {
    pub line_id: Uuid,
    pub product_id: Uuid,
    /// The batch it came out of at the sending outlet.
    pub batch_id: Uuid,
    pub quantity: Quantity,
}

impl PurchaseEvent {
    #[must_use]
    pub const fn order_id(&self) -> Uuid {
        match self {
            Self::Placed { order_id, .. }
            | Self::LineReceived { order_id, .. }
            | Self::Closed { order_id, .. } => *order_id,
        }
    }

    #[must_use]
    pub const fn at(&self) -> Timestamp {
        match self {
            Self::Placed { at, .. } | Self::LineReceived { at, .. } | Self::Closed { at, .. } => {
                *at
            }
        }
    }
}

impl TransferEvent {
    #[must_use]
    pub const fn transfer_id(&self) -> Uuid {
        match self {
            Self::Dispatched { transfer_id, .. }
            | Self::Received { transfer_id, .. }
            | Self::Settled { transfer_id, .. } => *transfer_id,
        }
    }

    #[must_use]
    pub const fn at(&self) -> Timestamp {
        match self {
            Self::Dispatched { at, .. } | Self::Received { at, .. } | Self::Settled { at, .. } => {
                *at
            }
        }
    }
}

impl EventPayload for PurchaseEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::Placed { .. } => "purchase.placed",
            Self::LineReceived { .. } => "purchase.line_received",
            Self::Closed { .. } => "purchase.closed",
        }
    }
}

impl EventPayload for TransferEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::Dispatched { .. } => "transfer.dispatched",
            Self::Received { .. } => "transfer.received",
            Self::Settled { .. } => "transfer.settled",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn kind_strings_are_stable_and_namespaced() {
        // Hashed into the chain; a rename invalidates every document already recorded.
        let placed = PurchaseEvent::Placed {
            order_id: id(1),
            supplier: "Karim Traders".to_owned(),
            reference: None,
            lines: Vec::new(),
            expected_at: None,
            at: Timestamp::from_millis(0),
            placed_by: id(2),
        };
        assert_eq!(placed.kind(), "purchase.placed");
        assert_eq!(placed.order_id(), id(1));

        let settled = TransferEvent::Settled {
            transfer_id: id(3),
            at: Timestamp::from_millis(0),
            settled_by: id(2),
        };
        assert_eq!(settled.kind(), "transfer.settled");
        assert_eq!(settled.transfer_id(), id(3));
    }

    #[test]
    fn a_receipt_carries_the_batch_it_became() {
        // The join between a document and the shelf. Without it a recall cannot walk back from a
        // lot to the invoice that brought it in.
        let received = PurchaseEvent::LineReceived {
            order_id: id(1),
            line_id: id(2),
            batch_id: id(3),
            quantity: Quantity::from_milli(5_000),
            unit_cost: Money::from_minor(4_200, Currency::Bdt),
            at: Timestamp::from_millis(0),
            received_by: id(4),
        };
        let encoded = serde_json::to_string(&received).expect("serialises");

        assert!(encoded.contains(r#""type":"line_received""#));
        assert_eq!(
            serde_json::from_str::<PurchaseEvent>(&encoded).expect("deserialises"),
            received
        );
    }
}
