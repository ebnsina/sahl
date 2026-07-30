use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventPayload;
use crate::money::{Currency, Money, Rounding};
use crate::quantity::Quantity;
use crate::tax::{Discount, PricingMode, TaxClass};
use crate::time::Timestamp;

use super::line::VoidReason;
use super::tender::TenderMethod;

/// Everything that can happen to a sale.
///
/// This is the write model: the sale's state is these events replayed, never a mutable row. That is
/// what makes the day auditable and what the fraud detection reads.
///
/// **The `kind` strings below are a wire format.** They are part of each event's hash, so renaming
/// one invalidates every chain containing it on every device in the field. Treat them as permanent.
///
/// The vocabulary is deliberately ticket-shaped rather than receipt-shaped — a sale is opened, lives
/// for a while, and is closed. Retail is the degenerate case where that lifetime is a few seconds;
/// a café ticket sits open for an hour. Building retail-first and bolting on tables later is how a
/// codebase ends up as two POS products, so the model is restaurant-grade from the start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SaleEvent {
    /// A ticket is started. For retail this is the moment the first item is scanned.
    Opened {
        sale_id: Uuid,
        /// The cashier or waiter who owns the ticket.
        opened_by: Uuid,
        currency: Currency,
        pricing_mode: PricingMode,
        rounding: Rounding,
    },

    /// An item is added, with its price, name and tax treatment snapshotted.
    LineAdded {
        sale_id: Uuid,
        line_id: Uuid,
        product_id: Uuid,
        name: String,
        unit_price: Money,
        quantity: Quantity,
        tax_class: TaxClass,
        /// Options chosen at the till. `default` so events written before modifiers existed still
        /// deserialize — and, because verification re-hashes the *stored* payload rather than a
        /// re-serialisation of this type, their recorded hashes stay valid.
        #[serde(default)]
        modifiers: Vec<crate::sale::Modifier>,
    },

    /// Quantity corrected — a mis-scan, or a weighed item re-weighed.
    LineQuantityChanged {
        sale_id: Uuid,
        line_id: Uuid,
        quantity: Quantity,
    },

    /// A line-level discount is applied.
    LineDiscounted {
        sale_id: Uuid,
        line_id: Uuid,
        discount: Discount,
        authorized_by: Uuid,
    },

    /// A line is voided. The line stays on the sale, flagged.
    LineVoided {
        sale_id: Uuid,
        line_id: Uuid,
        reason: VoidReason,
        authorized_by: Uuid,
    },

    /// A discount across the whole sale, apportioned back over lines at calculation time.
    OrderDiscounted {
        sale_id: Uuid,
        discount: Discount,
        authorized_by: Uuid,
    },

    /// A payment. Several may accumulate — split payment is ordinary.
    TenderRecorded {
        sale_id: Uuid,
        tender_id: Uuid,
        method: TenderMethod,
        amount: Money,
        reference: Option<String>,
    },

    /// The sale is closed and becomes an invoice. Nothing may be added afterwards.
    Completed {
        sale_id: Uuid,
        /// What the customer paid, snapshotted so a receipt reprint cannot drift from the original
        /// even if the VAT engine's rounding configuration later changes.
        total: Money,
        /// Cash handed back. Derived at completion and recorded, because the drawer count at shift
        /// close has to reconcile against what was actually given.
        change_given: Money,
        /// When it closed. Carried here rather than read from the envelope because this is what
        /// attributes the sale to a shift, and that must survive replay on any device.
        at: Timestamp,
    },

    /// A device takes ownership of the ticket.
    ///
    /// Carries `at` in the payload rather than reading the envelope, because the claim time is part
    /// of how a contest is resolved and must travel with the claim itself.
    TicketClaimed {
        sale_id: Uuid,
        device_id: Uuid,
        at: Timestamp,
    },

    /// The holder gives the ticket up — handing a table over, or closing a shift.
    TicketReleased { sale_id: Uuid, device_id: Uuid },

    /// The ticket was seated, or moved to another table.
    ///
    /// A separate event rather than a field on `Opened`, because a café takes an order before it
    /// knows where the party will sit as often as not, and moving a table mid-service is ordinary —
    /// a party of two joined by four more does not start a new ticket.
    ///
    /// Café only: `Capability::TableService`. A retail sale simply never carries one.
    Seated {
        sale_id: Uuid,
        table_id: Uuid,
        /// How many people. The denominator of every per-head figure a café cares about, and not
        /// derivable from the table's seat count — a two-seat table often holds three.
        covers: u32,
        at: Timestamp,
        seated_by: Uuid,
    },

    /// Lines were sent to a prep station.
    ///
    /// Recorded, not merely printed. Without it a second press of "send" reprints the whole order
    /// and the kitchen makes it twice — which is the single most expensive mistake a café POS can
    /// make, because the food is gone before anyone notices.
    ///
    /// Café only: `Capability::KitchenRouting`.
    LinesFired {
        sale_id: Uuid,
        line_ids: Vec<Uuid>,
        /// Which round. A cook reading "2" knows the first is already out.
        round: u32,
        at: Timestamp,
        fired_by: Uuid,
    },

    /// The ticket is dropped without payment — a walkout, or a cart abandoned at close.
    ///
    /// Recorded rather than deleted: an abandoned ticket full of scanned goods is itself a signal
    /// worth showing an owner.
    Abandoned { sale_id: Uuid, abandoned_by: Uuid },
}

impl SaleEvent {
    /// The sale this event belongs to.
    #[must_use]
    pub const fn sale_id(&self) -> Uuid {
        match self {
            Self::Opened { sale_id, .. }
            | Self::LineAdded { sale_id, .. }
            | Self::LineQuantityChanged { sale_id, .. }
            | Self::LineDiscounted { sale_id, .. }
            | Self::LineVoided { sale_id, .. }
            | Self::OrderDiscounted { sale_id, .. }
            | Self::TicketClaimed { sale_id, .. }
            | Self::TicketReleased { sale_id, .. }
            | Self::Seated { sale_id, .. }
            | Self::LinesFired { sale_id, .. }
            | Self::TenderRecorded { sale_id, .. }
            | Self::Completed { sale_id, .. }
            | Self::Abandoned { sale_id, .. } => *sale_id,
        }
    }
}

impl EventPayload for SaleEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::Opened { .. } => "sale.opened",
            Self::LineAdded { .. } => "sale.line_added",
            Self::LineQuantityChanged { .. } => "sale.line_quantity_changed",
            Self::LineDiscounted { .. } => "sale.line_discounted",
            Self::LineVoided { .. } => "sale.line_voided",
            Self::OrderDiscounted { .. } => "sale.order_discounted",
            Self::TicketClaimed { .. } => "sale.ticket_claimed",
            Self::TicketReleased { .. } => "sale.ticket_released",
            Self::Seated { .. } => "sale.seated",
            Self::LinesFired { .. } => "sale.lines_fired",
            Self::TenderRecorded { .. } => "sale.tender_recorded",
            Self::Completed { .. } => "sale.completed",
            Self::Abandoned { .. } => "sale.abandoned",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn every_variant_reports_its_sale() {
        assert_eq!(
            SaleEvent::Abandoned {
                sale_id: uuid(7),
                abandoned_by: uuid(1)
            }
            .sale_id(),
            uuid(7)
        );
    }

    #[test]
    fn kind_strings_are_stable_and_namespaced() {
        // These are hashed into the chain. Changing one invalidates every event of that type
        // already written to a merchant's device, so this test exists to make a rename loud.
        let opened = SaleEvent::Opened {
            sale_id: uuid(1),
            opened_by: uuid(2),
            currency: Currency::Bdt,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        };
        assert_eq!(opened.kind(), "sale.opened");

        let voided = SaleEvent::LineVoided {
            sale_id: uuid(1),
            line_id: uuid(3),
            reason: VoidReason::Mistake,
            authorized_by: uuid(2),
        };
        assert_eq!(voided.kind(), "sale.line_voided");
    }

    #[test]
    fn events_round_trip_through_json_with_a_type_tag() {
        let event = SaleEvent::TenderRecorded {
            sale_id: uuid(1),
            tender_id: uuid(4),
            method: TenderMethod::Cash,
            amount: Money::from_minor(50_000, Currency::Bdt),
            reference: None,
        };
        let encoded = serde_json::to_string(&event).expect("serialises");
        assert!(encoded.contains(r#""type":"tender_recorded""#));
        assert_eq!(
            serde_json::from_str::<SaleEvent>(&encoded).expect("deserialises"),
            event
        );
    }
}
