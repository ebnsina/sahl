//! The floor plan, rebuilt from events.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::event::FloorEvent;
use super::table::{FloorError, Table};

/// Every table the outlet has.
///
/// `BTreeMap` because this drives a floor plan a waiter learns the shape of. Hash order would
/// reshuffle the layout between launches, which is worse here than almost anywhere else: the whole
/// value of a floor plan is that table 4 is always in the same place on the screen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Floor {
    tables: BTreeMap<Uuid, Table>,
}

impl Floor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild from a stream of events.
    ///
    /// # Errors
    /// [`FloorError`] if the stream is inconsistent.
    pub fn replay(events: &[FloorEvent]) -> Result<Self, FloorError> {
        let mut floor = Self::new();
        for event in events {
            floor.apply(event)?;
        }
        Ok(floor)
    }

    /// Apply one event.
    ///
    /// # Errors
    /// [`FloorError`] if the event is not valid for the current state.
    pub fn apply(&mut self, event: &FloorEvent) -> Result<(), FloorError> {
        match event {
            FloorEvent::TableAdded { table_id, .. } => {
                if self.tables.contains_key(table_id) {
                    return Err(FloorError::Duplicate {
                        table_id: *table_id,
                    });
                }
                let table = event.to_table(true)?;
                self.assert_label_free(&table)?;
                self.tables.insert(*table_id, table);
            }

            FloorEvent::TableUpdated { table_id, .. } => {
                let active = self
                    .tables
                    .get(table_id)
                    .ok_or(FloorError::Unknown {
                        table_id: *table_id,
                    })?
                    .active;
                let table = event.to_table(active)?;
                self.assert_label_free(&table)?;
                self.tables.insert(*table_id, table);
            }

            FloorEvent::TableRemoved { table_id, .. } => {
                self.table_mut(*table_id)?.active = false;
            }

            FloorEvent::TableRestored { table_id, .. } => {
                self.table_mut(*table_id)?.active = true;
            }
        }

        Ok(())
    }

    /// Refuse a label already in use on another *active* table in the same section.
    ///
    /// Scoped to the section, because a real floor has a "1" on the terrace and a "1" inside, and
    /// staff say "terrace one" — the section is how they disambiguate, so the model should not
    /// pretend otherwise. Two tables called "4" *in the same section* is a floor where "put it on
    /// 4" is ambiguous, and staff resolve that by guessing.
    ///
    /// Removed tables are exempt: their label should be reusable when the furniture is replaced,
    /// which is the ordinary reason a table leaves service.
    fn assert_label_free(&self, table: &Table) -> Result<(), FloorError> {
        let clash = self.tables.values().any(|other| {
            other.id != table.id
                && other.active
                && other.section == table.section
                && other.label.eq_ignore_ascii_case(&table.label)
        });

        if clash {
            return Err(FloorError::DuplicateLabel {
                label: table.label.clone(),
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn get(&self, table_id: Uuid) -> Option<&Table> {
        self.tables.get(&table_id)
    }

    /// Tables in service, in the order a waiter reads them.
    ///
    /// Sorted by section then by label, and the label sorts numerically where it looks like a
    /// number — otherwise "10" lands between "1" and "2" and the plan stops matching the room.
    #[must_use]
    pub fn in_service(&self) -> Vec<&Table> {
        let mut found: Vec<&Table> = self.tables.values().filter(|table| table.active).collect();
        found.sort_by(|a, b| {
            a.section
                .cmp(&b.section)
                .then_with(|| label_key(&a.label).cmp(&label_key(&b.label)))
        });
        found
    }

    #[must_use]
    pub fn all(&self) -> Vec<&Table> {
        let mut found: Vec<&Table> = self.tables.values().collect();
        found.sort_by(|a, b| {
            a.section
                .cmp(&b.section)
                .then_with(|| label_key(&a.label).cmp(&label_key(&b.label)))
        });
        found
    }

    /// The sections in use, in display order.
    #[must_use]
    pub fn sections(&self) -> Vec<String> {
        let mut found: Vec<String> = self
            .tables
            .values()
            .filter(|table| table.active)
            .filter_map(|table| table.section.clone())
            .collect();
        found.sort();
        found.dedup();
        found
    }

    /// Total covers the room can seat.
    #[must_use]
    pub fn capacity(&self) -> u32 {
        self.tables
            .values()
            .filter(|table| table.active)
            .map(|table| table.seats)
            .sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    fn table_mut(&mut self, table_id: Uuid) -> Result<&mut Table, FloorError> {
        self.tables
            .get_mut(&table_id)
            .ok_or(FloorError::Unknown { table_id })
    }
}

/// Sort key that puts "10" after "9" rather than after "1".
///
/// A floor plan whose order does not match the room is a floor plan staff stop trusting, and table
/// labels are overwhelmingly numeric with the occasional "Bar 3".
fn label_key(label: &str) -> (u64, String) {
    let digits: String = label.chars().take_while(char::is_ascii_digit).collect();
    let number = digits.parse::<u64>().unwrap_or(u64::MAX);
    (number, label.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::super::event::TableDetails;
    use super::*;
    use crate::time::Timestamp;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(n: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + n)
    }

    fn added(table: u128, label: &str, section: Option<&str>, seats: u32) -> FloorEvent {
        FloorEvent::TableAdded {
            table_id: id(table),
            details: TableDetails {
                label: label.to_owned(),
                section: section.map(str::to_owned),
                seats,
            },
            at: at(0),
            added_by: id(0x0E),
        }
    }

    fn room() -> Floor {
        Floor::replay(&[
            added(1, "1", Some("Inside"), 2),
            added(2, "2", Some("Inside"), 4),
            added(3, "10", Some("Inside"), 6),
        ])
        .expect("valid")
    }

    #[test]
    fn tables_come_back_in_service() {
        let floor = room();
        assert_eq!(floor.in_service().len(), 3);
        assert_eq!(floor.capacity(), 12);
    }

    #[test]
    fn table_ten_sorts_after_table_nine_not_after_table_one() {
        // A plan whose order does not match the room is a plan staff stop trusting.
        let floor = Floor::replay(&[
            added(1, "1", None, 2),
            added(2, "10", None, 2),
            added(3, "2", None, 2),
            added(4, "9", None, 2),
        ])
        .expect("valid");

        let labels: Vec<&str> = floor
            .in_service()
            .iter()
            .map(|table| table.label.as_str())
            .collect();
        assert_eq!(labels, vec!["1", "2", "9", "10"]);
    }

    #[test]
    fn tables_group_by_section() {
        let floor = Floor::replay(&[
            added(1, "1", Some("Terrace"), 2),
            added(2, "1", Some("Inside"), 2),
        ])
        .expect("valid");

        assert_eq!(floor.sections(), vec!["Inside", "Terrace"]);
        assert_eq!(
            floor.in_service()[0].section.as_deref(),
            Some("Inside"),
            "sections sort before labels"
        );
    }

    #[test]
    fn the_same_label_in_two_sections_is_fine() {
        // A real floor has a "1" on the terrace and a "1" inside. Staff say "terrace one", so the
        // section is how they disambiguate and the model should not pretend otherwise.
        assert!(
            Floor::replay(&[
                added(1, "1", Some("Terrace"), 2),
                added(2, "1", Some("Inside"), 2),
            ])
            .is_ok()
        );
    }

    #[test]
    fn two_active_tables_in_one_section_cannot_share_a_label() {
        // Now it *is* ambiguous, and staff resolve ambiguity by guessing.
        let result = Floor::replay(&[added(1, "4", None, 2), added(2, "4", None, 4)]);
        assert_eq!(
            result,
            Err(FloorError::DuplicateLabel {
                label: "4".to_owned()
            })
        );
    }

    #[test]
    fn a_label_clash_ignores_case() {
        let result = Floor::replay(&[added(1, "Bar 3", None, 2), added(2, "bar 3", None, 2)]);
        assert!(matches!(result, Err(FloorError::DuplicateLabel { .. })));
    }

    #[test]
    fn a_removed_table_frees_its_label_for_the_furniture_that_replaces_it() {
        let mut floor = room();
        floor
            .apply(&FloorEvent::TableRemoved {
                table_id: id(1),
                at: at(10),
                removed_by: id(0x0E),
            })
            .expect("removes");

        assert!(
            floor.apply(&added(9, "1", Some("Inside"), 2)).is_ok(),
            "the label is reusable once the table is out of service"
        );
    }

    #[test]
    fn a_removed_table_leaves_the_plan_but_not_the_record() {
        // Past tickets reference it; a ticket on a table that no longer exists cannot be reported on.
        let mut floor = room();
        floor
            .apply(&FloorEvent::TableRemoved {
                table_id: id(1),
                at: at(10),
                removed_by: id(0x0E),
            })
            .expect("removes");

        assert_eq!(floor.in_service().len(), 2);
        assert_eq!(floor.all().len(), 3);
        assert_eq!(floor.capacity(), 10, "and it stops counting toward covers");
        assert_eq!(floor.get(id(1)).expect("present").label, "1");
    }

    #[test]
    fn a_restored_table_returns() {
        let mut floor = room();
        for event in [
            FloorEvent::TableRemoved {
                table_id: id(1),
                at: at(10),
                removed_by: id(0x0E),
            },
            FloorEvent::TableRestored {
                table_id: id(1),
                at: at(11),
                restored_by: id(0x0E),
            },
        ] {
            floor.apply(&event).expect("applies");
        }
        assert_eq!(floor.in_service().len(), 3);
    }

    #[test]
    fn adding_the_same_table_twice_is_refused() {
        let result = Floor::replay(&[added(1, "4", None, 2), added(1, "5", None, 2)]);
        assert_eq!(result, Err(FloorError::Duplicate { table_id: id(1) }));
    }

    #[test]
    fn replay_is_deterministic() {
        // This is the floor plan's layout. A room that reshuffles between launches is a room staff
        // cannot learn.
        let events = vec![
            added(3, "10", Some("Inside"), 6),
            added(1, "1", Some("Inside"), 2),
            FloorEvent::TableRemoved {
                table_id: id(3),
                at: at(10),
                removed_by: id(0x0E),
            },
        ];
        assert_eq!(
            Floor::replay(&events).expect("valid"),
            Floor::replay(&events).expect("valid")
        );
    }
}
