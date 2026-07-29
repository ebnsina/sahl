//! Fiscal events.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventPayload;
use crate::tax::OrderTotals;
use crate::time::Timestamp;

use super::chain::InvoiceSeal;

/// What an invoice is, for hashing purposes.
///
/// The totals rather than the whole sale. A regime cares that the money on the invoice has not
/// changed; the line-by-line record of how it got there is the event log's job, and hashing both
/// would bind the fiscal chain to details that can legitimately be re-projected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceContent {
    pub totals: OrderTotals,
    /// The regime in force when it was issued. Part of the hash because the same money under a
    /// different regime is a different document.
    pub regime: String,
}

/// Everything that happens to the fiscal sequence.
///
/// Kind strings are hashed into the event chain, so they are a wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FiscalEvent {
    /// A sale took its place in the fiscal sequence.
    ///
    /// Recorded separately from `sale.completed` even though they always happen together, because
    /// they answer to different authorities: one is the shop's record of a transaction, the other
    /// is the state's record of an invoice. A sale can be corrected by a later event; its invoice
    /// number can never be reused.
    InvoiceIssued {
        seal: InvoiceSeal,
        content: InvoiceContent,
        at: Timestamp,
        issued_by: Uuid,
    },
}

impl FiscalEvent {
    #[must_use]
    pub const fn sale_id(&self) -> Uuid {
        match self {
            Self::InvoiceIssued { seal, .. } => seal.sale_id,
        }
    }

    #[must_use]
    pub const fn at(&self) -> Timestamp {
        match self {
            Self::InvoiceIssued { at, .. } => *at,
        }
    }
}

impl EventPayload for FiscalEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::InvoiceIssued { .. } => "fiscal.invoice_issued",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::FiscalChain;
    use crate::money::{Currency, Money};
    use crate::quantity::Quantity;
    use crate::tax::{LineInput, OrderInput, TaxClass, calculate};

    #[test]
    fn kind_strings_are_stable_and_namespaced() {
        // Hashed into the event chain; a rename invalidates every invoice already recorded.
        let totals = calculate(&OrderInput::new(
            Currency::Bdt,
            vec![LineInput::new(
                Money::from_minor(11_500, Currency::Bdt),
                Quantity::ONE,
                TaxClass::standard(1500),
            )],
        ))
        .expect("calculates");

        let content = InvoiceContent {
            totals,
            regime: "bd_mushak".to_owned(),
        };
        let mut chain = FiscalChain::new(Uuid::from_u128(3));
        let seal = chain
            .seal(Uuid::from_u128(1), Timestamp::from_millis(0), &content)
            .expect("seals");

        let event = FiscalEvent::InvoiceIssued {
            seal,
            content,
            at: Timestamp::from_millis(0),
            issued_by: Uuid::from_u128(0xCA),
        };

        assert_eq!(event.kind(), "fiscal.invoice_issued");
        assert_eq!(event.sale_id(), Uuid::from_u128(1));

        let encoded = serde_json::to_string(&event).expect("serialises");
        assert!(encoded.contains(r#""type":"invoice_issued""#));
        assert_eq!(
            serde_json::from_str::<FiscalEvent>(&encoded).expect("deserialises"),
            event
        );
    }
}
