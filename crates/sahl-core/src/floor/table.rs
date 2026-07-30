//! Tables.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FloorError {
    #[error("no table {table_id}")]
    Unknown { table_id: Uuid },

    #[error("table {table_id} already exists")]
    Duplicate { table_id: Uuid },

    #[error("a table needs a label")]
    Blank,

    #[error("{label} is already the label of another table")]
    DuplicateLabel { label: String },

    #[error("a table cannot seat {seats}")]
    BadSeats { seats: u32 },
}

/// The most people one table can seat.
///
/// Not a technical limit — a guard against a typo. Someone entering "40" for table 4 would make
/// every cover-count report meaningless, and nobody has a forty-seat table.
pub const MAX_SEATS: u32 = 30;

/// A table on the floor.
///
/// Furniture, not a transaction. Which ticket is on it is derived from the open sales, because a
/// table that stored its own ticket id would need to be kept in step with the sale — and the two
/// disagreeing is how a café ends up unable to seat a table it can see is empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub id: Uuid,
    /// What the staff call it: "4", "T12", "Terrace 2". Not a number — plenty of floors use letters.
    pub label: String,
    /// Which part of the floor. Drives the layout of the floor plan, and lets one waiter's section
    /// be filtered from another's.
    pub section: Option<String>,
    pub seats: u32,
    /// Whether it is in service. A removed table stays in the catalogue of tables because past
    /// tickets reference it, and a ticket on a table that no longer exists cannot be reported on.
    pub active: bool,
}

impl Table {
    /// # Errors
    /// [`FloorError`] naming what is wrong.
    pub fn validate(&self) -> Result<(), FloorError> {
        if self.label.trim().is_empty() {
            return Err(FloorError::Blank);
        }
        if self.seats == 0 || self.seats > MAX_SEATS {
            return Err(FloorError::BadSeats { seats: self.seats });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        Table {
            id: Uuid::from_u128(1),
            label: "4".to_owned(),
            section: Some("Terrace".to_owned()),
            seats: 4,
            active: true,
        }
    }

    #[test]
    fn a_complete_table_is_accepted() {
        assert_eq!(table().validate(), Ok(()));
    }

    #[test]
    fn a_table_needs_a_label() {
        let unlabelled = Table {
            label: "  ".to_owned(),
            ..table()
        };
        assert_eq!(unlabelled.validate(), Err(FloorError::Blank));
    }

    #[test]
    fn a_label_can_be_letters() {
        // Plenty of floors use "T12" or "Bar 3". Forcing a number would fit a spreadsheet, not a
        // café.
        let lettered = Table {
            label: "Bar 3".to_owned(),
            ..table()
        };
        assert_eq!(lettered.validate(), Ok(()));
    }

    #[test]
    fn a_table_with_no_seats_is_refused() {
        let empty = Table {
            seats: 0,
            ..table()
        };
        assert!(matches!(empty.validate(), Err(FloorError::BadSeats { .. })));
    }

    #[test]
    fn an_implausible_seat_count_is_refused() {
        // A typo of 40 for table 4 would make every cover-count report meaningless.
        let huge = Table {
            seats: 40,
            ..table()
        };
        assert!(matches!(huge.validate(), Err(FloorError::BadSeats { .. })));
    }
}
