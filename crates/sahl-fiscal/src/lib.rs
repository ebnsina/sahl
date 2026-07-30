//! # sahl-fiscal
//!
//! What a jurisdiction requires on top of a sale.
//!
//! Every market wants the same facts arranged differently and numbered its own way, so the seam is
//! a trait rather than a branch in the sale code. A completed sale goes in; a document that a
//! jurisdiction recognises comes out. The sale never knows which country it is in.
//!
//! Like `sahl-core`, this crate is I/O-free and async-free. Producing a document is pure; *sending*
//! one — ZATCA's reporting API, an EFD box on a serial port — belongs to whatever holds the socket.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
    )
)]

pub mod bd_mushak;
pub mod noop;
pub mod zatca;

use sahl_core::money::MoneyError;
use sahl_core::tax::OrderTotals;
use sahl_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FiscalError {
    #[error("arithmetic error: {0}")]
    Money(#[from] MoneyError),

    #[error("{field} is required to issue a {document}")]
    Missing {
        field: &'static str,
        document: &'static str,
    },

    #[error("a fiscal document cannot be issued for a sale with no lines")]
    Empty,

    #[error("{0}")]
    Invalid(String),
}

/// Who is issuing, as the jurisdiction has registered them.
///
/// Held separately from the sale because it is outlet configuration, not transaction data — and
/// because getting it wrong is a compliance failure on every invoice at once rather than one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seller {
    pub name: String,
    /// Bangladesh: the 13-digit BIN. Saudi: the 15-digit VAT registration number.
    pub registration: String,
    /// Where the document is issued from, which is not always the registered address.
    pub address: String,
}

/// Who is buying, when the jurisdiction requires them named.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Buyer {
    pub name: Option<String>,
    pub registration: Option<String>,
    pub address: Option<String>,
}

/// One line, as the fiscal layer sees it.
///
/// Carries the description and unit alongside the computed amounts because a fiscal document names
/// what was sold, and "SKU-4471" is not a description any inspector accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalLine {
    pub description: String,
    /// "pcs", "kg", "litre" — the unit the quantity is counted in.
    pub unit: String,
    /// Thousandths of a unit.
    pub quantity_milli: i64,
}

/// A completed sale, ready to become a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    pub sale_id: Uuid,
    /// Per-device monotonic counter. Both target regimes need one, and neither accepts a gap.
    pub sequence: u64,
    pub issued_at: Timestamp,
    pub seller: Seller,
    pub buyer: Buyer,
    /// Line descriptions, in the same order as `totals.lines`.
    pub lines: Vec<FiscalLine>,
    pub totals: OrderTotals,
    /// Where the goods went. Mushak 6.3 has a field for it; ZATCA does not.
    pub destination: Option<String>,
}

/// What a jurisdiction requires of a sale.
pub trait Fiscalization {
    /// A stable identifier for the regime, for logs and configuration.
    fn regime(&self) -> &'static str;

    /// Turn a completed sale into the document this jurisdiction recognises.
    ///
    /// # Errors
    /// [`FiscalError`] if the sale or the registration details cannot produce a valid document.
    fn issue(&self, invoice: &Invoice) -> Result<Document, FiscalError>;
}

/// A document some regime recognises.
///
/// An enum rather than a boxed trait object: the set of regimes is small, closed, and known at
/// compile time, and a caller that wants to print a Mushak needs its actual columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "regime", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Document {
    /// Bangladesh: the Mushak 6.3 tax challan.
    ///
    /// Boxed: a challan carries ten columns per line, and leaving it inline would make every
    /// `Document` — including `None` — as large as the largest regime ever added.
    BdMushak63(Box<bd_mushak::Mushak63>),
    /// Saudi Arabia: the simplified tax invoice, Phase 1.
    Zatca(Box<zatca::SimplifiedTaxInvoice>),
    /// No fiscal regime configured. An ordinary receipt is the whole obligation.
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sahl_core::money::{Currency, Money};
    use sahl_core::quantity::Quantity;
    use sahl_core::tax::{Discount, LineInput, OrderInput, TaxClass, calculate};

    pub(crate) fn seller() -> Seller {
        Seller {
            name: "Karim Store".to_owned(),
            registration: "0031234567890".to_owned(),
            address: "12 Dhanmondi 27, Dhaka 1209".to_owned(),
        }
    }

    #[test]
    fn a_document_round_trips_through_json() {
        // Documents reach the sync payload and the printer, so the wire format has to survive.
        let none = Document::None;
        let encoded = serde_json::to_string(&none).expect("serialises");

        assert!(encoded.contains(r#""regime":"none""#));
        assert_eq!(
            serde_json::from_str::<Document>(&encoded).expect("deserialises"),
            none
        );
    }

    /// A one-line tax-inclusive sale, the ordinary Bangladeshi retail case.
    pub(crate) fn invoice(sequence: u64) -> Invoice {
        let totals = calculate(&OrderInput::new(
            Currency::Bdt,
            vec![LineInput {
                unit_price: Money::from_minor(11_500, Currency::Bdt),
                quantity: Quantity::ONE,
                tax_class: TaxClass::standard(1500),
                discount: Discount::None,
            }],
        ))
        .expect("calculates");

        Invoice {
            sale_id: Uuid::from_u128(1),
            sequence,
            issued_at: Timestamp::from_millis(1_753_000_000_000),
            seller: seller(),
            buyer: Buyer::default(),
            lines: vec![FiscalLine {
                description: "Basmati rice 5kg".to_owned(),
                unit: "pcs".to_owned(),
                quantity_milli: 1_000,
            }],
            totals,
            destination: None,
        }
    }
}
