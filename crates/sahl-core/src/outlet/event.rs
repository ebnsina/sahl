//! Outlet configuration events.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventPayload;
use crate::money::Currency;
use crate::time::Timestamp;

use super::config::{FiscalRegime, OutletConfig, OutletError};
use super::profile::Profile;
use crate::scale::ScaleFormat;
use crate::staff::ApprovalPolicy;

/// The settings an outlet is configured with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutletSettings {
    pub name: String,
    pub profile: Profile,
    pub currency: Currency,
    pub timezone: String,
    pub regime: FiscalRegime,
    pub tax_registration: Option<String>,
    pub address: String,
    /// How this outlet's counter scale lays out its printed labels. Grocery only, and absent
    /// everywhere else — a shop with no scale must not be asked to describe one.
    #[serde(default)]
    pub scale: Option<ScaleFormat>,
    /// Absent on an event written before thresholds existed, which reads as the strictest setting
    /// — the safe direction for a default to fall.
    #[serde(default)]
    pub approval: Option<ApprovalPolicy>,
}

/// Everything that happens to an outlet's configuration.
///
/// Kind strings are hashed into the chain, so they are a wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutletEvent {
    /// The outlet was set up, or its settings were changed.
    ///
    /// One event for both. A settings change is a full replacement rather than a patch, because a
    /// patch that arrives out of order leaves an outlet in a state nobody chose — and these arrive
    /// from a dashboard that may be hours ahead of a till that was offline.
    Configured {
        outlet_id: Uuid,
        settings: OutletSettings,
        at: Timestamp,
        configured_by: Uuid,
    },
}

impl OutletEvent {
    #[must_use]
    pub const fn outlet_id(&self) -> Uuid {
        match self {
            Self::Configured { outlet_id, .. } => *outlet_id,
        }
    }

    #[must_use]
    pub const fn at(&self) -> Timestamp {
        match self {
            Self::Configured { at, .. } => *at,
        }
    }

    /// The configuration this event describes.
    ///
    /// # Errors
    /// [`OutletError`] if the settings would not be valid to trade under.
    pub fn to_config(&self) -> Result<OutletConfig, OutletError> {
        match self {
            Self::Configured {
                outlet_id,
                settings,
                at,
                ..
            } => {
                let config = OutletConfig {
                    outlet_id: *outlet_id,
                    name: settings.name.clone(),
                    profile: settings.profile,
                    currency: settings.currency,
                    timezone: settings.timezone.clone(),
                    regime: settings.regime,
                    tax_registration: settings.tax_registration.clone(),
                    address: settings.address.clone(),
                    scale: settings.scale.clone(),
                    approval: settings
                        .approval
                        .unwrap_or_else(|| ApprovalPolicy::strictest(settings.currency)),
                    configured_at: *at,
                };
                config.validate()?;
                Ok(config)
            }
        }
    }
}

impl EventPayload for OutletEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::Configured { .. } => "outlet.configured",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> OutletSettings {
        OutletSettings {
            name: "Karim Store".to_owned(),
            profile: Profile::Retail,
            currency: Currency::Bdt,
            timezone: "Asia/Dhaka".to_owned(),
            regime: FiscalRegime::BdMushak,
            tax_registration: Some("0031234567890".to_owned()),
            address: "12 Dhanmondi 27, Dhaka".to_owned(),
            scale: None,
            approval: None,
        }
    }

    fn configured(settings: OutletSettings) -> OutletEvent {
        OutletEvent::Configured {
            outlet_id: Uuid::from_u128(2),
            settings,
            at: Timestamp::from_millis(1_753_000_000_000),
            configured_by: Uuid::from_u128(0x0E),
        }
    }

    #[test]
    fn kind_strings_are_stable_and_namespaced() {
        let event = configured(settings());
        assert_eq!(event.kind(), "outlet.configured");
        assert_eq!(event.outlet_id(), Uuid::from_u128(2));

        let encoded = serde_json::to_string(&event).expect("serialises");
        assert!(encoded.contains(r#""type":"configured""#));
        assert!(encoded.contains(r#""profile":"retail""#));
        assert_eq!(
            serde_json::from_str::<OutletEvent>(&encoded).expect("deserialises"),
            event
        );
    }

    #[test]
    fn a_valid_event_yields_a_configuration() {
        let config = configured(settings()).to_config().expect("valid");
        assert_eq!(config.profile, Profile::Retail);
        assert_eq!(config.regime, FiscalRegime::BdMushak);
    }

    #[test]
    fn an_event_that_would_not_trade_is_refused_on_replay() {
        // A till must not adopt a configuration it cannot issue documents under, even if some
        // dashboard managed to emit one.
        let event = configured(OutletSettings {
            tax_registration: None,
            ..settings()
        });

        assert!(matches!(
            event.to_config(),
            Err(OutletError::RegistrationRequired { .. })
        ));
    }
}
