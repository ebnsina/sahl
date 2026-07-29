//! Staff as events.
//!
//! Staff records go through the event log rather than a separate table synced some other way. That
//! is not tidiness — it means a till that has been offline for a week still learns about the person
//! hired on Tuesday through the same push-pull it already runs, and a role change is dated and
//! attributed like every other fact in the system.
//!
//! The PIN hash travels. It has to: the terminal verifies offline, so it must hold the hash. That
//! is what an Argon2id hash is *for*, and the local store is encrypted at rest besides. The PIN
//! itself never appears here or anywhere else.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventPayload;
use crate::time::Timestamp;

use super::role::Role;

/// Everything that happens to a staff member.
///
/// Kind strings are hashed into the chain, so they are a wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StaffEvent {
    /// Someone joined and can now sign in.
    Enrolled {
        staff_id: Uuid,
        name: String,
        role: Role,
        /// Argon2id PHC string. Never the PIN.
        pin_hash: String,
        at: Timestamp,
        enrolled_by: Uuid,
    },

    /// A promotion or demotion.
    ///
    /// Recorded rather than overwritten, because "was this person a manager when they approved
    /// that void" is a question an audit has to be able to answer months later.
    RoleChanged {
        staff_id: Uuid,
        role: Role,
        at: Timestamp,
        changed_by: Uuid,
    },

    /// A new PIN. The old hash is superseded, not deleted — the log is append-only.
    PinChanged {
        staff_id: Uuid,
        pin_hash: String,
        at: Timestamp,
        changed_by: Uuid,
    },

    /// They left, or their access was withdrawn.
    ///
    /// Deactivation rather than deletion. Their name still has to render against every void they
    /// approved, and a sale attributed to a blank is a sale nobody can ask about.
    Deactivated {
        staff_id: Uuid,
        at: Timestamp,
        deactivated_by: Uuid,
    },

    /// Access restored.
    Reactivated {
        staff_id: Uuid,
        at: Timestamp,
        reactivated_by: Uuid,
    },
}

impl StaffEvent {
    #[must_use]
    pub const fn staff_id(&self) -> Uuid {
        match self {
            Self::Enrolled { staff_id, .. }
            | Self::RoleChanged { staff_id, .. }
            | Self::PinChanged { staff_id, .. }
            | Self::Deactivated { staff_id, .. }
            | Self::Reactivated { staff_id, .. } => *staff_id,
        }
    }

    #[must_use]
    pub const fn at(&self) -> Timestamp {
        match self {
            Self::Enrolled { at, .. }
            | Self::RoleChanged { at, .. }
            | Self::PinChanged { at, .. }
            | Self::Deactivated { at, .. }
            | Self::Reactivated { at, .. } => *at,
        }
    }
}

impl EventPayload for StaffEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::Enrolled { .. } => "staff.enrolled",
            Self::RoleChanged { .. } => "staff.role_changed",
            Self::PinChanged { .. } => "staff.pin_changed",
            Self::Deactivated { .. } => "staff.deactivated",
            Self::Reactivated { .. } => "staff.reactivated",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn kind_strings_are_stable_and_namespaced() {
        // Hashed into the chain; a rename invalidates every staff record already synced.
        let enrolled = StaffEvent::Enrolled {
            staff_id: id(1),
            name: "Ruma".to_owned(),
            role: Role::Cashier,
            pin_hash: "$argon2id$v=19$...".to_owned(),
            at: Timestamp::from_millis(0),
            enrolled_by: id(2),
        };
        assert_eq!(enrolled.kind(), "staff.enrolled");
        assert_eq!(enrolled.staff_id(), id(1));
    }

    #[test]
    fn a_role_serialises_as_the_string_the_database_stores() {
        let changed = StaffEvent::RoleChanged {
            staff_id: id(1),
            role: Role::Manager,
            at: Timestamp::from_millis(0),
            changed_by: id(2),
        };
        let encoded = serde_json::to_string(&changed).expect("serialises");

        assert!(encoded.contains(r#""role":"manager""#));
        assert_eq!(
            serde_json::from_str::<StaffEvent>(&encoded).expect("deserialises"),
            changed
        );
    }
}
