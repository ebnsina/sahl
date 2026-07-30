//! Who is standing at the till.
//!
//! Until now the till knew what *role* had approved something and nothing about who was ringing
//! the sale — the sell screen carried a constant. That made two things impossible: attributing a
//! sale to a person honestly, and letting a cashier do anything on their own authority, because
//! the till could not tell whose authority it was.
//!
//! ## Ephemeral on purpose
//!
//! A session is device state, not a business fact, so nothing here is written to the event log.
//! The log already records who opened each sale; a parallel stream of sign-ins would be a second
//! account of the same thing, able to disagree with the first. It also means a restart signs
//! everybody out, which is the behaviour a shared till wants anyway.
//!
//! ## Expiry is derived, never timed
//!
//! Idleness is a function of the clock at the moment somebody asks, not a timer that fires. A
//! timer does not run while the machine is asleep, and a till that wakes still signed in as
//! whoever left at closing is precisely the hole this closes.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::time::Timestamp;

use super::role::Role;

/// How long a till stays signed in after the last thing anybody did.
///
/// Five minutes: shorter than the ticket lease, because a lease protects an order from being taken
/// and a session protects the shop from whoever walks past an unattended counter. Tuned by
/// observation, not derivation — expect this to change once real counters are watched.
pub const SESSION_IDLE_TIMEOUT_MILLIS: i64 = 5 * 60 * 1_000;

/// Somebody signed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub staff_id: Uuid,
    /// Carried so a caller need not look it up again and risk reading a role that changed
    /// mid-shift. Re-resolved by [`Presence::current`] on every read for exactly that reason.
    pub role: Role,
    pub signed_in_at: Timestamp,
    /// Last thing this person did. Idleness is measured from here, not from sign-in.
    pub touched_at: Timestamp,
}

impl Session {
    #[must_use]
    pub const fn new(staff_id: Uuid, role: Role, at: Timestamp) -> Self {
        Self {
            staff_id,
            role,
            signed_in_at: at,
            touched_at: at,
        }
    }

    /// Whether this session has gone idle by `now`.
    #[must_use]
    pub fn is_expired(&self, now: Timestamp, timeout_millis: i64) -> bool {
        now.millis()
            .saturating_sub(self.touched_at.millis())
            .saturating_sub(timeout_millis)
            > 0
    }
}

/// Whether anybody is at the till.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Presence {
    #[default]
    SignedOut,
    SignedIn(Session),
}

impl Presence {
    #[must_use]
    pub const fn sign_in(staff_id: Uuid, role: Role, at: Timestamp) -> Self {
        Self::SignedIn(Session::new(staff_id, role, at))
    }

    /// Who is signed in as of `now`, or nobody.
    ///
    /// Reading is what expires a session: nothing here runs on a clock, so a till that was asleep
    /// for an hour reports nobody the moment it is asked.
    #[must_use]
    pub fn current(&self, now: Timestamp, timeout_millis: i64) -> Option<Session> {
        match self {
            Self::SignedOut => None,
            Self::SignedIn(session) => {
                (!session.is_expired(now, timeout_millis)).then_some(*session)
            }
        }
    }

    /// Record that the signed-in person did something, pushing back the idle clock.
    ///
    /// An already-expired session is not revived. Someone who walked away and came back signs in
    /// again — reviving it would mean the timeout only applied to tills nobody touched, which is
    /// the opposite of the point.
    pub fn touch(&mut self, now: Timestamp, timeout_millis: i64) {
        if let Self::SignedIn(session) = self {
            if session.is_expired(now, timeout_millis) {
                *self = Self::SignedOut;
            } else {
                session.touched_at = now;
            }
        }
    }

    pub const fn sign_out(&mut self) {
        *self = Self::SignedOut;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: i64 = SESSION_IDLE_TIMEOUT_MILLIS;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + millis)
    }

    fn signed_in() -> Presence {
        Presence::sign_in(id(0xCA), Role::Cashier, at(0))
    }

    #[test]
    fn nobody_is_signed_in_to_begin_with() {
        assert_eq!(Presence::default().current(at(0), TIMEOUT), None);
    }

    #[test]
    fn signing_in_names_the_person_and_their_role() {
        let session = signed_in().current(at(0), TIMEOUT).expect("present");

        assert_eq!(session.staff_id, id(0xCA));
        assert_eq!(session.role, Role::Cashier);
    }

    #[test]
    fn a_till_left_alone_signs_itself_out() {
        // The hole this closes: whoever walks past an unattended counter.
        assert_eq!(signed_in().current(at(TIMEOUT + 1), TIMEOUT), None);
    }

    #[test]
    fn a_session_at_exactly_the_timeout_is_still_good() {
        // Off by one here logs somebody out mid-transaction on a slow evening.
        assert!(signed_in().current(at(TIMEOUT), TIMEOUT).is_some());
    }

    #[test]
    fn doing_something_pushes_the_idle_clock_back() {
        let mut presence = signed_in();
        presence.touch(at(TIMEOUT - 1), TIMEOUT);

        assert!(
            presence.current(at(TIMEOUT + 1), TIMEOUT).is_some(),
            "still within the timeout of the last action"
        );
    }

    #[test]
    fn an_expired_session_is_not_revived_by_activity() {
        // Otherwise the timeout would only apply to tills nobody touched, which is the opposite of
        // the point — the person who walks up is not the person who walked away.
        let mut presence = signed_in();
        presence.touch(at(TIMEOUT + 1), TIMEOUT);

        assert_eq!(presence, Presence::SignedOut);
        assert_eq!(presence.current(at(TIMEOUT + 1), TIMEOUT), None);
    }

    #[test]
    fn expiry_is_read_rather_than_timed() {
        // A till asleep for an hour reports nobody the moment it is asked. Nothing had to fire
        // while it was off.
        let presence = signed_in();
        assert!(presence.current(at(0), TIMEOUT).is_some());
        assert!(presence.current(at(60 * 60 * 1_000), TIMEOUT).is_none());
    }

    #[test]
    fn signing_out_is_immediate_rather_than_waiting_for_the_timeout() {
        let mut presence = signed_in();
        presence.sign_out();

        assert_eq!(presence.current(at(0), TIMEOUT), None);
    }

    #[test]
    fn idleness_is_measured_from_the_last_action_not_from_signing_in() {
        let mut presence = signed_in();
        for step in 1..10 {
            presence.touch(at(step * (TIMEOUT / 2)), TIMEOUT);
        }

        assert!(
            presence.current(at(9 * (TIMEOUT / 2)), TIMEOUT).is_some(),
            "a busy hour does not sign somebody out"
        );
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_expire_a_session() {
        // Device clocks are corrected by NTP mid-shift. Signing somebody out because time moved
        // backwards would be an outage nobody could explain.
        assert!(signed_in().current(at(-60_000), TIMEOUT).is_some());
    }
}
