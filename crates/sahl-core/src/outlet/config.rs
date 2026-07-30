//! What an outlet is configured as.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::money::Currency;
use crate::time::Timestamp;

use super::profile::Profile;
use crate::scale::ScaleFormat;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OutletError {
    #[error("this outlet has not been set up")]
    NotConfigured,

    #[error("{field} cannot be blank")]
    Blank { field: &'static str },

    #[error("{regime} requires a tax registration number")]
    RegistrationRequired { regime: &'static str },

    #[error("unknown fiscal regime {0}")]
    UnknownRegime(String),
}

/// Which jurisdiction's rules this outlet trades under.
///
/// A closed set rather than a free string: an outlet configured with a regime nothing implements
/// would trade for a month before anyone noticed no documents were being produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FiscalRegime {
    /// Not VAT-registered, or a market Sahl has not been localised for. A real deployment: the
    /// shop owes its customer a receipt and the state nothing extra.
    None,
    /// Bangladesh — Mushak 6.3.
    BdMushak,
    /// Saudi Arabia — ZATCA, Phase 1.
    Zatca,
}

impl FiscalRegime {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::BdMushak => "bd_mushak",
            Self::Zatca => "zatca",
        }
    }

    /// Whether this regime needs a tax registration number before it can issue anything.
    #[must_use]
    pub const fn needs_registration(self) -> bool {
        matches!(self, Self::BdMushak | Self::Zatca)
    }

    /// Parse a stored or transmitted label.
    ///
    /// # Errors
    /// [`OutletError::UnknownRegime`] rather than a silent fallback to `None` — a typo that quietly
    /// disabled fiscal documents would be discovered by an inspector, not by us.
    pub fn from_label(label: &str) -> Result<Self, OutletError> {
        match label {
            "none" => Ok(Self::None),
            "bd_mushak" => Ok(Self::BdMushak),
            "zatca" => Ok(Self::Zatca),
            other => Err(OutletError::UnknownRegime(other.to_owned())),
        }
    }
}

/// How an outlet trades.
///
/// Everything here is set at onboarding and changes rarely, but each field is a thing that must be
/// right on every invoice at once — so it is validated when it is set, not when it is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutletConfig {
    pub outlet_id: Uuid,
    pub name: String,
    pub profile: Profile,
    pub currency: Currency,
    /// IANA timezone. A POS reports by business day, and the device clock is not the authority —
    /// a chain with a till in Dhaka and one in Riyadh closes them on different days.
    pub timezone: String,
    pub regime: FiscalRegime,
    /// BIN in Bangladesh, VAT number in the Gulf. Required by any regime that issues documents.
    pub tax_registration: Option<String>,
    /// Where documents are issued from, which is not always the registered address.
    pub address: String,
    /// Set only where there is a scale printing labels. [`None`] means every barcode is an
    /// ordinary one, which is the correct reading for a shop that has never weighed anything.
    pub scale: Option<ScaleFormat>,
    pub configured_at: Timestamp,
}

impl OutletConfig {
    /// Check the configuration is one this outlet can actually trade under.
    ///
    /// # Errors
    /// [`OutletError`] naming the field that is wrong.
    pub fn validate(&self) -> Result<(), OutletError> {
        if self.name.trim().is_empty() {
            return Err(OutletError::Blank { field: "name" });
        }
        if self.timezone.trim().is_empty() {
            return Err(OutletError::Blank { field: "timezone" });
        }
        if self.address.trim().is_empty() {
            return Err(OutletError::Blank { field: "address" });
        }
        if self.regime.needs_registration()
            && self
                .tax_registration
                .as_ref()
                .is_none_or(|value| value.trim().is_empty())
        {
            // Refused at configuration rather than at the first sale. A till that accepts a blank
            // BIN trades all morning and then cannot issue a single valid challan for the day.
            return Err(OutletError::RegistrationRequired {
                regime: self.regime.label(),
            });
        }
        Ok(())
    }

    /// Whether this outlet has the capability, by its profile.
    #[must_use]
    pub const fn can(&self, capability: super::profile::Capability) -> bool {
        self.profile.can(capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> OutletConfig {
        OutletConfig {
            outlet_id: Uuid::from_u128(1),
            name: "Karim Store — Dhanmondi".to_owned(),
            profile: Profile::Retail,
            currency: Currency::Bdt,
            timezone: "Asia/Dhaka".to_owned(),
            regime: FiscalRegime::BdMushak,
            tax_registration: Some("0031234567890".to_owned()),
            address: "12 Dhanmondi 27, Dhaka 1209".to_owned(),
            scale: None,
            configured_at: Timestamp::from_millis(1_753_000_000_000),
        }
    }

    #[test]
    fn a_complete_configuration_is_accepted() {
        assert_eq!(config().validate(), Ok(()));
    }

    #[test]
    fn a_mushak_outlet_without_a_bin_is_refused_at_setup() {
        // Refused now rather than at the first sale: a till that accepts a blank BIN trades all
        // morning and then cannot issue a single valid challan for the day.
        let outlet = OutletConfig {
            tax_registration: None,
            ..config()
        };

        assert_eq!(
            outlet.validate(),
            Err(OutletError::RegistrationRequired {
                regime: "bd_mushak"
            })
        );
    }

    #[test]
    fn a_whitespace_bin_is_not_a_bin() {
        let outlet = OutletConfig {
            tax_registration: Some("   ".to_owned()),
            ..config()
        };
        assert!(outlet.validate().is_err());
    }

    #[test]
    fn an_unregistered_outlet_needs_no_number() {
        // A shop below the VAT threshold is a real deployment, not a misconfiguration.
        let outlet = OutletConfig {
            regime: FiscalRegime::None,
            tax_registration: None,
            ..config()
        };
        assert_eq!(outlet.validate(), Ok(()));
    }

    #[test]
    fn a_blank_address_is_refused() {
        // Mushak 6.3 has a "Challan Issuing Address" field, and a blank one voids the document.
        let outlet = OutletConfig {
            address: "  ".to_owned(),
            ..config()
        };
        assert_eq!(
            outlet.validate(),
            Err(OutletError::Blank { field: "address" })
        );
    }

    #[test]
    fn a_blank_timezone_is_refused() {
        // Falling back to the device clock mis-assigns evening sales to the wrong business day.
        let outlet = OutletConfig {
            timezone: String::new(),
            ..config()
        };
        assert_eq!(
            outlet.validate(),
            Err(OutletError::Blank { field: "timezone" })
        );
    }

    #[test]
    fn an_unknown_regime_is_refused_rather_than_defaulted() {
        // A typo that quietly disabled fiscal documents would be found by an inspector, not by us.
        assert_eq!(
            FiscalRegime::from_label("bd_mushaq"),
            Err(OutletError::UnknownRegime("bd_mushaq".to_owned()))
        );
        assert_eq!(FiscalRegime::from_label("none"), Ok(FiscalRegime::None));
        assert_eq!(
            FiscalRegime::from_label("bd_mushak"),
            Ok(FiscalRegime::BdMushak)
        );
    }

    #[test]
    fn capabilities_come_from_the_profile() {
        use super::super::profile::Capability;

        let cafe = OutletConfig {
            profile: Profile::Cafe,
            ..config()
        };
        assert!(cafe.can(Capability::OpenTickets));
        assert!(!config().can(Capability::OpenTickets));
    }
}
