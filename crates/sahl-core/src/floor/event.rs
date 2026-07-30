//! Floor plan events.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventPayload;
use crate::time::Timestamp;

use super::table::{FloorError, Table};

/// The editable facts about a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDetails {
    pub label: String,
    pub section: Option<String>,
    pub seats: u32,
}

/// Everything that happens to the floor plan.
///
/// Kind strings are hashed into the chain, so they are a wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FloorEvent {
    TableAdded {
        table_id: Uuid,
        details: TableDetails,
        at: Timestamp,
        added_by: Uuid,
    },

    /// Relabelled, moved to another section, or re-seated.
    ///
    /// A full replacement rather than a patch, for the same reason as the catalogue: two devices
    /// editing while apart cannot have patches merged into a state either intended.
    TableUpdated {
        table_id: Uuid,
        details: TableDetails,
        at: Timestamp,
        updated_by: Uuid,
    },

    /// Taken out of service.
    ///
    /// Removed rather than deleted: past tickets reference it, and a ticket on a table that no
    /// longer exists cannot be reported on.
    TableRemoved {
        table_id: Uuid,
        at: Timestamp,
        removed_by: Uuid,
    },

    TableRestored {
        table_id: Uuid,
        at: Timestamp,
        restored_by: Uuid,
    },
}

impl FloorEvent {
    #[must_use]
    pub const fn table_id(&self) -> Uuid {
        match self {
            Self::TableAdded { table_id, .. }
            | Self::TableUpdated { table_id, .. }
            | Self::TableRemoved { table_id, .. }
            | Self::TableRestored { table_id, .. } => *table_id,
        }
    }

    #[must_use]
    pub const fn at(&self) -> Timestamp {
        match self {
            Self::TableAdded { at, .. }
            | Self::TableUpdated { at, .. }
            | Self::TableRemoved { at, .. }
            | Self::TableRestored { at, .. } => *at,
        }
    }

    /// The table these details describe.
    ///
    /// # Errors
    /// [`FloorError`] if the details would not be a usable table.
    pub fn to_table(&self, active: bool) -> Result<Table, FloorError> {
        match self {
            Self::TableAdded {
                table_id, details, ..
            }
            | Self::TableUpdated {
                table_id, details, ..
            } => {
                let table = Table {
                    id: *table_id,
                    label: details.label.trim().to_owned(),
                    section: details.section.clone(),
                    seats: details.seats,
                    active,
                };
                table.validate()?;
                Ok(table)
            }
            Self::TableRemoved { table_id, .. } | Self::TableRestored { table_id, .. } => {
                Err(FloorError::Unknown {
                    table_id: *table_id,
                })
            }
        }
    }
}

impl EventPayload for FloorEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::TableAdded { .. } => "floor.table_added",
            Self::TableUpdated { .. } => "floor.table_updated",
            Self::TableRemoved { .. } => "floor.table_removed",
            Self::TableRestored { .. } => "floor.table_restored",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_strings_are_stable_and_namespaced() {
        // Hashed into the chain; a rename invalidates every floor edit already recorded.
        let added = FloorEvent::TableAdded {
            table_id: Uuid::from_u128(1),
            details: TableDetails {
                label: "4".to_owned(),
                section: Some("Terrace".to_owned()),
                seats: 4,
            },
            at: Timestamp::from_millis(0),
            added_by: Uuid::from_u128(2),
        };

        assert_eq!(added.kind(), "floor.table_added");
        assert_eq!(added.table_id(), Uuid::from_u128(1));

        let encoded = serde_json::to_string(&added).expect("serialises");
        assert!(encoded.contains(r#""type":"table_added""#));
        assert_eq!(
            serde_json::from_str::<FloorEvent>(&encoded).expect("deserialises"),
            added
        );
    }

    #[test]
    fn an_unusable_table_is_refused_on_replay() {
        let bad = FloorEvent::TableAdded {
            table_id: Uuid::from_u128(1),
            details: TableDetails {
                label: String::new(),
                section: None,
                seats: 4,
            },
            at: Timestamp::from_millis(0),
            added_by: Uuid::from_u128(2),
        };
        assert_eq!(bad.to_table(true), Err(FloorError::Blank));
    }
}
