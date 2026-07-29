use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventPayload;
use crate::money::Money;
use crate::quantity::Quantity;
use crate::time::Timestamp;

/// Why stock left a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IssueReason {
    Sale,
    /// Spoiled, broken, or expired and written off.
    Wastage,
    /// Sent to another outlet.
    TransferOut,
    /// Returned to the supplier.
    ReturnToSupplier,
    /// Taken for the shop's own use.
    Internal,
}

/// Why stock came back into a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReturnReason {
    /// A customer brought it back.
    CustomerRefund,
    /// Arrived from another outlet.
    TransferIn,
}

/// Everything that moves stock.
///
/// Kind strings are hashed into the chain, so they are a wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InventoryEvent {
    /// A delivery arrived and became a batch.
    ///
    /// Receiving *creates* the batch rather than adding to an existing one, because a second
    /// delivery of the same product is a different lot with its own expiry — merging them is
    /// exactly what makes a recall under-report.
    BatchReceived {
        batch_id: Uuid,
        product_id: Uuid,
        lot: Option<String>,
        expires_at: Option<Timestamp>,
        quantity: Quantity,
        /// What it cost per unit. Kept per batch because the same product bought twice at
        /// different prices has two margins, and averaging them hides which purchase was bad.
        unit_cost: Money,
        supplier: Option<String>,
        at: Timestamp,
        received_by: Uuid,
    },

    /// Stock left a batch.
    StockIssued {
        batch_id: Uuid,
        quantity: Quantity,
        reason: IssueReason,
        /// The sale it belonged to, when there was one — what a recall traces along.
        sale_id: Option<Uuid>,
        at: Timestamp,
        issued_by: Uuid,
    },

    /// Stock came back into a batch.
    StockReturned {
        batch_id: Uuid,
        quantity: Quantity,
        reason: ReturnReason,
        sale_id: Option<Uuid>,
        at: Timestamp,
        returned_by: Uuid,
    },

    /// A physical count of one batch.
    ///
    /// Absolute, not a delta: a count is "there are seven here", and the adjustment is derived.
    /// Recording a delta instead would mean someone had to do the subtraction, which is both a
    /// place to make a mistake and a place to hide one.
    BatchCounted {
        batch_id: Uuid,
        counted: Quantity,
        at: Timestamp,
        counted_by: Uuid,
    },
}

impl InventoryEvent {
    #[must_use]
    pub const fn batch_id(&self) -> Uuid {
        match self {
            Self::BatchReceived { batch_id, .. }
            | Self::StockIssued { batch_id, .. }
            | Self::StockReturned { batch_id, .. }
            | Self::BatchCounted { batch_id, .. } => *batch_id,
        }
    }

    #[must_use]
    pub const fn at(&self) -> Timestamp {
        match self {
            Self::BatchReceived { at, .. }
            | Self::StockIssued { at, .. }
            | Self::StockReturned { at, .. }
            | Self::BatchCounted { at, .. } => *at,
        }
    }
}

impl EventPayload for InventoryEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::BatchReceived { .. } => "inventory.batch_received",
            Self::StockIssued { .. } => "inventory.stock_issued",
            Self::StockReturned { .. } => "inventory.stock_returned",
            Self::BatchCounted { .. } => "inventory.batch_counted",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    #[test]
    fn kind_strings_are_stable_and_namespaced() {
        // Hashed into the chain; a rename invalidates every movement already recorded.
        let received = InventoryEvent::BatchReceived {
            batch_id: Uuid::from_u128(1),
            product_id: Uuid::from_u128(2),
            lot: None,
            expires_at: None,
            quantity: Quantity::ONE,
            unit_cost: Money::from_minor(100, Currency::Bdt),
            supplier: None,
            at: Timestamp::from_millis(0),
            received_by: Uuid::from_u128(3),
        };
        assert_eq!(received.kind(), "inventory.batch_received");
        assert_eq!(received.batch_id(), Uuid::from_u128(1));
    }

    #[test]
    fn an_issue_carries_the_sale_a_recall_would_trace() {
        let issued = InventoryEvent::StockIssued {
            batch_id: Uuid::from_u128(1),
            quantity: Quantity::ONE,
            reason: IssueReason::Sale,
            sale_id: Some(Uuid::from_u128(9)),
            at: Timestamp::from_millis(0),
            issued_by: Uuid::from_u128(3),
        };
        let encoded = serde_json::to_string(&issued).expect("serialises");

        assert!(encoded.contains(r#""type":"stock_issued""#));
        assert!(encoded.contains(r#""reason":"sale""#));
        assert_eq!(
            serde_json::from_str::<InventoryEvent>(&encoded).expect("deserialises"),
            issued
        );
    }
}
