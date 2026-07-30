//! Where an item is made.

use serde::{Deserialize, Serialize};

/// The prep station a line is routed to.
///
/// A closed set rather than a free string. Stations are physical — there is a printer bolted to
/// each one — so a typo would route an order to a station that does not exist and the food would
/// simply never be made, with nothing on any screen to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Station {
    /// Hot food.
    Kitchen,
    /// Drinks that are poured rather than cooked.
    Bar,
    /// Coffee, tea.
    Counter,
    /// Made cold — salads, desserts already plated.
    Pass,
}

impl Station {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Kitchen => "kitchen",
            Self::Bar => "bar",
            Self::Counter => "counter",
            Self::Pass => "pass",
        }
    }

    /// As printed at the top of the ticket, where a cook reads it across a room.
    #[must_use]
    pub const fn heading(self) -> &'static str {
        match self {
            Self::Kitchen => "KITCHEN",
            Self::Bar => "BAR",
            Self::Counter => "COUNTER",
            Self::Pass => "PASS",
        }
    }

    /// Parse a stored label.
    ///
    /// # Errors
    /// The unrecognised label, so a caller can say which one it was. Never falls back to a default:
    /// an item silently routed to the kitchen instead of the bar is a drink nobody pours.
    pub fn from_label(label: &str) -> Result<Self, String> {
        match label {
            "kitchen" => Ok(Self::Kitchen),
            "bar" => Ok(Self::Bar),
            "counter" => Ok(Self::Counter),
            "pass" => Ok(Self::Pass),
            other => Err(other.to_owned()),
        }
    }

    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Kitchen, Self::Bar, Self::Counter, Self::Pass]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stations_round_trip_through_their_stored_label() {
        for station in Station::all() {
            assert_eq!(Station::from_label(station.label()), Ok(station));
        }
    }

    #[test]
    fn an_unknown_station_is_refused_rather_than_defaulted() {
        // An item silently routed to the kitchen instead of the bar is a drink nobody pours, and
        // nothing on any screen would say so.
        assert_eq!(Station::from_label("grill"), Err("grill".to_owned()));
    }

    #[test]
    fn headings_are_shouted_because_they_are_read_across_a_room() {
        assert_eq!(Station::Kitchen.heading(), "KITCHEN");
        assert_eq!(Station::Bar.heading(), "BAR");
    }
}
