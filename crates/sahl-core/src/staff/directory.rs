//! Who works here, rebuilt from staff events.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::time::Timestamp;

use super::event::StaffEvent;
use super::pin;
use super::role::{Permission, Role};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DirectoryError {
    #[error("no staff member {staff_id}")]
    Unknown { staff_id: Uuid },

    #[error("staff member {staff_id} was already enrolled")]
    Duplicate { staff_id: Uuid },

    #[error("{0}")]
    Pin(#[from] pin::PinError),
}

/// One person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffMember {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
    pub active: bool,
    /// Argon2id PHC string. Serialised because the terminal needs it to verify offline; never
    /// rendered, never logged.
    pin_hash: String,
    pub enrolled_at: Timestamp,
}

impl StaffMember {
    #[must_use]
    pub const fn can(&self, permission: Permission) -> bool {
        // An inactive account has no permissions at all, whatever its role says. Checking `active`
        // at every call site instead is the version where one call site forgets.
        self.active && self.role.can(permission)
    }
}

/// The outlet's staff.
///
/// `BTreeMap` because this reaches reports and sync payloads, where hash order would differ between
/// processes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directory {
    members: BTreeMap<Uuid, StaffMember>,
}

/// Why a sign-in attempt failed.
///
/// One variant for "no such person" and "wrong PIN" would be safer against enumeration, but this is
/// a shop till where the staff list is on the wall — and a cashier who mistyped needs to be told
/// that rather than left wondering if they have been removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    /// Authenticated. Carries the role so a caller need not look it up again and risk a mismatch.
    Ok {
        staff_id: Uuid,
        role: Role,
    },
    WrongPin,
    Unknown,
    /// The account exists but has been deactivated.
    Inactive,
}

impl Directory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild from a stream of events.
    ///
    /// # Errors
    /// [`DirectoryError`] if the stream is inconsistent.
    pub fn replay(events: &[StaffEvent]) -> Result<Self, DirectoryError> {
        let mut directory = Self::new();
        for event in events {
            directory.apply(event)?;
        }
        Ok(directory)
    }

    /// Apply one event.
    ///
    /// # Errors
    /// [`DirectoryError`] if the event is not valid for the current state.
    pub fn apply(&mut self, event: &StaffEvent) -> Result<(), DirectoryError> {
        match event {
            StaffEvent::Enrolled {
                staff_id,
                name,
                role,
                pin_hash,
                at,
                ..
            } => {
                if self.members.contains_key(staff_id) {
                    return Err(DirectoryError::Duplicate {
                        staff_id: *staff_id,
                    });
                }
                self.members.insert(
                    *staff_id,
                    StaffMember {
                        id: *staff_id,
                        name: name.clone(),
                        role: *role,
                        active: true,
                        pin_hash: pin_hash.clone(),
                        enrolled_at: *at,
                    },
                );
            }

            StaffEvent::RoleChanged { staff_id, role, .. } => {
                self.member_mut(*staff_id)?.role = *role;
            }

            StaffEvent::PinChanged {
                staff_id, pin_hash, ..
            } => {
                self.member_mut(*staff_id)?.pin_hash = pin_hash.clone();
            }

            StaffEvent::Deactivated { staff_id, .. } => {
                self.member_mut(*staff_id)?.active = false;
            }

            StaffEvent::Reactivated { staff_id, .. } => {
                self.member_mut(*staff_id)?.active = true;
            }
        }

        Ok(())
    }

    /// Check a PIN against one person.
    ///
    /// # Errors
    /// [`DirectoryError::Pin`] only if the stored hash is unreadable — a wrong PIN is
    /// [`SignIn::WrongPin`], not an error.
    pub fn sign_in(&self, staff_id: Uuid, entered: &str) -> Result<SignIn, DirectoryError> {
        let Some(member) = self.members.get(&staff_id) else {
            return Ok(SignIn::Unknown);
        };
        if !member.active {
            // Checked before the PIN so a departed employee's correct PIN is never a success, even
            // momentarily, on any code path.
            return Ok(SignIn::Inactive);
        }
        if pin::verify(entered, &member.pin_hash)? {
            Ok(SignIn::Ok {
                staff_id,
                role: member.role,
            })
        } else {
            Ok(SignIn::WrongPin)
        }
    }

    /// Authenticate someone senior enough to approve `permission`.
    ///
    /// Takes the whole directory rather than a named person because the cashier at the counter does
    /// not know which manager is nearby — they hand the till over and someone types their own PIN.
    ///
    /// # Errors
    /// [`DirectoryError::Pin`] if a stored hash is unreadable.
    pub fn approve(&self, permission: Permission, entered: &str) -> Result<SignIn, DirectoryError> {
        let mut seen = false;
        for member in self
            .members
            .values()
            .filter(|member| member.can(permission))
        {
            seen = true;
            if pin::verify(entered, &member.pin_hash)? {
                return Ok(SignIn::Ok {
                    staff_id: member.id,
                    role: member.role,
                });
            }
        }
        // "Nobody here can approve that" and "you typed it wrong" are different problems, and only
        // one of them is solved by trying again.
        Ok(if seen {
            SignIn::WrongPin
        } else {
            SignIn::Unknown
        })
    }

    #[must_use]
    pub fn get(&self, staff_id: Uuid) -> Option<&StaffMember> {
        self.members.get(&staff_id)
    }

    /// The role of one person, for judging an audit entry after the fact.
    #[must_use]
    pub fn role_of(&self, staff_id: Uuid) -> Option<Role> {
        self.members.get(&staff_id).map(|member| member.role)
    }

    /// Everyone who can currently sign in, in id order.
    #[must_use]
    pub fn active(&self) -> Vec<&StaffMember> {
        self.members
            .values()
            .filter(|member| member.active)
            .collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    fn member_mut(&mut self, staff_id: Uuid) -> Result<&mut StaffMember, DirectoryError> {
        self.members
            .get_mut(&staff_id)
            .ok_or(DirectoryError::Unknown { staff_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::password_hash::SaltString;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(n: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + n)
    }

    /// Argon2 requires at least 8 bytes of salt, so short names are padded.
    fn salt(seed: &str) -> SaltString {
        let padded = format!("{seed}-sahl-test");
        SaltString::encode_b64(padded.as_bytes()).expect("valid salt")
    }

    const RUMA: u128 = 0xCA;
    const HABIB: u128 = 0x11A;
    const OWNER: u128 = 0x0E;

    fn enrolled(who: u128, name: &str, role: Role, secret: &str) -> StaffEvent {
        StaffEvent::Enrolled {
            staff_id: id(who),
            name: name.to_owned(),
            role,
            pin_hash: pin::hash(secret, &salt(name)).expect("hashes"),
            at: at(0),
            enrolled_by: id(OWNER),
        }
    }

    fn shop() -> Directory {
        Directory::replay(&[
            enrolled(RUMA, "Ruma", Role::Cashier, "8317"),
            enrolled(HABIB, "Habib", Role::Manager, "5294"),
        ])
        .expect("valid")
    }

    #[test]
    fn a_correct_pin_signs_someone_in_with_their_role() {
        assert_eq!(
            shop().sign_in(id(RUMA), "8317"),
            Ok(SignIn::Ok {
                staff_id: id(RUMA),
                role: Role::Cashier
            })
        );
    }

    #[test]
    fn a_wrong_pin_is_refused_without_being_an_error() {
        assert_eq!(shop().sign_in(id(RUMA), "0000"), Ok(SignIn::WrongPin));
    }

    #[test]
    fn an_unknown_person_is_refused() {
        assert_eq!(shop().sign_in(id(0xFFFF), "8317"), Ok(SignIn::Unknown));
    }

    #[test]
    fn a_manager_pin_approves_what_a_cashier_may_not_do() {
        // The cashier hands the till over; the manager types their own PIN. Nobody has to know in
        // advance which manager is on the floor.
        assert_eq!(
            shop().approve(Permission::VoidLine, "5294"),
            Ok(SignIn::Ok {
                staff_id: id(HABIB),
                role: Role::Manager
            })
        );
    }

    #[test]
    fn a_cashier_pin_does_not_approve_a_managers_action() {
        // The whole control. If this passed, the approval prompt would be theatre.
        assert_eq!(
            shop().approve(Permission::VoidLine, "8317"),
            Ok(SignIn::WrongPin)
        );
    }

    #[test]
    fn approval_reports_nobody_available_differently_from_a_typo() {
        // Only one of those is solved by trying again.
        let cashiers_only =
            Directory::replay(&[enrolled(RUMA, "Ruma", Role::Cashier, "8317")]).expect("valid");

        assert_eq!(
            cashiers_only.approve(Permission::VoidLine, "8317"),
            Ok(SignIn::Unknown)
        );
    }

    #[test]
    fn a_deactivated_account_cannot_sign_in_with_a_correct_pin() {
        let mut directory = shop();
        directory
            .apply(&StaffEvent::Deactivated {
                staff_id: id(RUMA),
                at: at(10),
                deactivated_by: id(OWNER),
            })
            .expect("applies");

        assert_eq!(directory.sign_in(id(RUMA), "8317"), Ok(SignIn::Inactive));
        assert_eq!(directory.active().len(), 1);
    }

    #[test]
    fn a_deactivated_manager_cannot_approve() {
        // Someone who left the company on Friday must not still be authorising voids on Monday.
        let mut directory = shop();
        directory
            .apply(&StaffEvent::Deactivated {
                staff_id: id(HABIB),
                at: at(10),
                deactivated_by: id(OWNER),
            })
            .expect("applies");

        assert_eq!(
            directory.approve(Permission::VoidLine, "5294"),
            Ok(SignIn::Unknown),
            "no active approver remains"
        );
    }

    #[test]
    fn a_departed_member_is_still_nameable() {
        // Their name has to render against every void they approved; a sale attributed to a blank
        // is a sale nobody can ask about.
        let mut directory = shop();
        directory
            .apply(&StaffEvent::Deactivated {
                staff_id: id(RUMA),
                at: at(10),
                deactivated_by: id(OWNER),
            })
            .expect("applies");

        assert_eq!(directory.get(id(RUMA)).expect("present").name, "Ruma");
        assert_eq!(directory.role_of(id(RUMA)), Some(Role::Cashier));
    }

    #[test]
    fn a_promotion_takes_effect() {
        let mut directory = shop();
        directory
            .apply(&StaffEvent::RoleChanged {
                staff_id: id(RUMA),
                role: Role::Manager,
                at: at(10),
                changed_by: id(OWNER),
            })
            .expect("applies");

        assert_eq!(
            directory.approve(Permission::VoidLine, "8317"),
            Ok(SignIn::Ok {
                staff_id: id(RUMA),
                role: Role::Manager
            })
        );
    }

    #[test]
    fn a_new_pin_replaces_the_old_one() {
        let mut directory = shop();
        directory
            .apply(&StaffEvent::PinChanged {
                staff_id: id(RUMA),
                pin_hash: pin::hash("4471", &salt("Ruma2")).expect("hashes"),
                at: at(10),
                changed_by: id(OWNER),
            })
            .expect("applies");

        assert_eq!(
            directory.sign_in(id(RUMA), "4471").expect("checks"),
            SignIn::Ok {
                staff_id: id(RUMA),
                role: Role::Cashier
            }
        );
        assert_eq!(directory.sign_in(id(RUMA), "8317"), Ok(SignIn::WrongPin));
    }

    #[test]
    fn enrolling_the_same_person_twice_is_refused() {
        let result = Directory::replay(&[
            enrolled(RUMA, "Ruma", Role::Cashier, "8317"),
            enrolled(RUMA, "Ruma", Role::Manager, "5294"),
        ]);
        assert_eq!(
            result,
            Err(DirectoryError::Duplicate { staff_id: id(RUMA) })
        );
    }

    #[test]
    fn changing_an_unknown_person_is_refused() {
        let mut directory = shop();
        let result = directory.apply(&StaffEvent::RoleChanged {
            staff_id: id(0xFFFF),
            role: Role::Owner,
            at: at(10),
            changed_by: id(OWNER),
        });
        assert_eq!(
            result,
            Err(DirectoryError::Unknown {
                staff_id: id(0xFFFF)
            })
        );
    }

    #[test]
    fn replay_is_deterministic() {
        let events = vec![
            enrolled(RUMA, "Ruma", Role::Cashier, "8317"),
            enrolled(HABIB, "Habib", Role::Manager, "5294"),
            StaffEvent::RoleChanged {
                staff_id: id(RUMA),
                role: Role::Manager,
                at: at(10),
                changed_by: id(OWNER),
            },
        ];

        assert_eq!(
            Directory::replay(&events).expect("valid"),
            Directory::replay(&events).expect("valid")
        );
    }
}
