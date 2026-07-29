//! Vertical profiles as data.
//!
//! One codebase serves retail, café and grocery. The profile is a row, not a branch — the moment a
//! vertical needs a fork in the core, the capability is wrong and wants redesigning rather than an
//! `if profile == Cafe` somewhere in the sale code.
//!
//! Retail is the degenerate café: a ticket that opens and closes in one gesture. Building the order
//! model restaurant-grade once and letting retail use the simple path is far cheaper than two order
//! models that must agree about money.

use serde::{Deserialize, Serialize};

/// What kind of shop this outlet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Profile {
    /// Scan, tender, done. Tickets open and close in one gesture.
    Retail,
    /// Tables, courses, and tickets that stay open for an hour.
    Cafe,
    /// Retail plus weighed goods, batches and expiry dates.
    Grocery,
}

/// Something a profile may or may not do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Capability {
    /// Tickets stay open across many interactions and belong to a table.
    OpenTickets,
    /// A floor plan with tables to seat and move.
    TableService,
    /// Kitchen order tickets routed to a prep station.
    KitchenRouting,
    /// Modifiers and options on a line — "no ice", "extra shot".
    LineModifiers,
    /// Firing courses in sequence rather than all at once.
    CourseFiring,
    /// Splitting a ticket across several payers.
    SplitBills,
    /// Fractional quantities from a scale.
    WeighedItems,
    /// Reading a connected scale directly.
    ScaleIntegration,
    /// Batches with expiry dates, and FEFO picking.
    BatchExpiry,
    /// A cash drawer at all. Rare to disable, but a delivery-only kitchen has none.
    CashDrawer,
}

impl Profile {
    /// Whether this profile has `capability`.
    ///
    /// Exhaustive per profile rather than a set of defaults with overrides: an outlet reading a
    /// capability must get the same answer everywhere, and a default that drifts from an override
    /// is the bug this design exists to prevent.
    #[must_use]
    pub const fn can(self, capability: Capability) -> bool {
        use Capability as C;
        match self {
            // Everything a counter needs and nothing a kitchen does.
            Self::Retail => matches!(capability, C::CashDrawer),

            Self::Cafe => matches!(
                capability,
                C::OpenTickets
                    | C::TableService
                    | C::KitchenRouting
                    | C::LineModifiers
                    | C::CourseFiring
                    | C::SplitBills
                    | C::CashDrawer
            ),

            // Retail plus the things that make food retail different: scales and expiry.
            Self::Grocery => matches!(
                capability,
                C::WeighedItems | C::ScaleIntegration | C::BatchExpiry | C::CashDrawer
            ),
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Retail => "retail",
            Self::Cafe => "cafe",
            Self::Grocery => "grocery",
        }
    }

    /// Every capability this profile has, in a stable order.
    #[must_use]
    pub fn capabilities(self) -> Vec<Capability> {
        use Capability as C;
        [
            C::OpenTickets,
            C::TableService,
            C::KitchenRouting,
            C::LineModifiers,
            C::CourseFiring,
            C::SplitBills,
            C::WeighedItems,
            C::ScaleIntegration,
            C::BatchExpiry,
            C::CashDrawer,
        ]
        .into_iter()
        .filter(|capability| self.can(*capability))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retail_outlet_has_no_kitchen() {
        assert!(!Profile::Retail.can(Capability::KitchenRouting));
        assert!(!Profile::Retail.can(Capability::TableService));
        assert!(!Profile::Retail.can(Capability::OpenTickets));
        assert!(Profile::Retail.can(Capability::CashDrawer));
    }

    #[test]
    fn a_cafe_keeps_tickets_open() {
        // The one capability the whole order model was built restaurant-grade for.
        assert!(Profile::Cafe.can(Capability::OpenTickets));
        assert!(Profile::Cafe.can(Capability::SplitBills));
        assert!(Profile::Cafe.can(Capability::CourseFiring));
    }

    #[test]
    fn a_grocery_weighs_and_tracks_expiry() {
        assert!(Profile::Grocery.can(Capability::WeighedItems));
        assert!(Profile::Grocery.can(Capability::BatchExpiry));
        assert!(!Profile::Grocery.can(Capability::KitchenRouting));
    }

    #[test]
    fn every_profile_has_a_drawer() {
        for profile in [Profile::Retail, Profile::Cafe, Profile::Grocery] {
            assert!(profile.can(Capability::CashDrawer), "{profile:?}");
        }
    }

    #[test]
    fn capabilities_are_listed_in_a_stable_order() {
        // This reaches the UI and the sync payload; a set would order differently per process.
        assert_eq!(Profile::Cafe.capabilities(), Profile::Cafe.capabilities());
        assert_eq!(
            Profile::Cafe.capabilities().first(),
            Some(&Capability::OpenTickets)
        );
    }

    #[test]
    fn labels_match_what_the_database_stores() {
        // The `outlet.profile` CHECK constraint lists these exact strings.
        for (profile, stored) in [
            (Profile::Retail, "retail"),
            (Profile::Cafe, "cafe"),
            (Profile::Grocery, "grocery"),
        ] {
            assert_eq!(profile.label(), stored);
            assert_eq!(
                serde_json::to_string(&profile).expect("serialises"),
                format!("\"{stored}\"")
            );
        }
    }
}
