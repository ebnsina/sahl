use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::Money;

/// A mobile wallet. Which ones matter is entirely regional, which is why this is its own enum
/// rather than a free-text field: a typo'd provider name in a settlement report is a reconciliation
/// problem nobody enjoys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Wallet {
    /// Bangladesh — by far the most common non-cash tender in the launch market.
    Bkash,
    Nagad,
    Rocket,
    Upay,
    /// Saudi Arabia.
    StcPay,
}

/// How a customer paid.
///
/// Kept closed rather than open so that a new tender type is a deliberate change with a migration,
/// not a string that silently appears in a shift report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TenderMethod {
    Cash,
    Card,
    MobileWallet {
        wallet: Wallet,
    },
    BankTransfer,
    /// On account — the customer owes it. Common in neighbourhood retail in both markets.
    StoreCredit,
}

impl TenderMethod {
    /// Whether a customer may hand over more than the total and receive change.
    ///
    /// Only cash. A card or wallet is charged the exact amount, so an over-tender there is not
    /// generosity — it is a data-entry error, and silently issuing change against it would take
    /// real money out of the drawer for a payment that never arrived.
    #[must_use]
    pub const fn accepts_overtender(self) -> bool {
        matches!(self, Self::Cash)
    }

    /// Whether this tender puts physical money in the drawer.
    ///
    /// Drives the drawer kick and the expected cash figure at shift close.
    #[must_use]
    pub const fn affects_cash_drawer(self) -> bool {
        matches!(self, Self::Cash)
    }
}

/// One payment against a sale. A sale may have several — split payments are ordinary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tender {
    pub id: Uuid,
    pub method: TenderMethod,
    /// What the customer handed over. For cash this may exceed the balance due; change is derived,
    /// never stored, so it cannot disagree with the arithmetic.
    pub amount: Money,
    /// Provider or terminal reference — a wallet transaction id, card auth code. Retained for
    /// reconciliation and dispute handling.
    pub reference: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cash_accepts_an_overtender() {
        assert!(TenderMethod::Cash.accepts_overtender());
        assert!(!TenderMethod::Card.accepts_overtender());
        assert!(
            !TenderMethod::MobileWallet {
                wallet: Wallet::Bkash
            }
            .accepts_overtender()
        );
        assert!(!TenderMethod::BankTransfer.accepts_overtender());
        assert!(!TenderMethod::StoreCredit.accepts_overtender());
    }

    #[test]
    fn only_cash_touches_the_drawer() {
        assert!(TenderMethod::Cash.affects_cash_drawer());
        assert!(!TenderMethod::Card.affects_cash_drawer());
    }

    #[test]
    fn tender_methods_round_trip_through_json() {
        // These land in the event log, so their encoding is a wire format.
        let wallet = TenderMethod::MobileWallet {
            wallet: Wallet::Nagad,
        };
        let encoded = serde_json::to_string(&wallet).expect("serialises");
        assert_eq!(encoded, r#"{"method":"mobile_wallet","wallet":"nagad"}"#);
        assert_eq!(
            serde_json::from_str::<TenderMethod>(&encoded).expect("deserialises"),
            wallet
        );
    }
}
