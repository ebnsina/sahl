//! Who owns an open ticket.
//!
//! ## What a lease can and cannot promise
//!
//! Two waiters must not both fire the same kitchen order. Online that is easy — the server
//! arbitrates. Offline it is **impossible**: two disconnected devices cannot agree on anything, so
//! any design claiming to prevent a double claim while both are offline is lying.
//!
//! What this design does instead, in order of importance:
//!
//! 1. **Shrinks the window.** A ticket stays leased to its device until it goes idle, so the only
//!    way two devices both believe they hold it is a genuine abandonment plus an outage.
//! 2. **Makes a double claim detectable.** Both claims reach the server, and the loser is visible
//!    rather than silently merged away.
//! 3. **Resolves it identically everywhere.** Both devices and the server reach the same winner
//!    from the same events, so nobody has to ask which copy is right.
//!
//! Irreversible side effects — firing a course to the kitchen, printing a bill — must therefore
//! check the lease *and* be prepared for a compensating event when they lose. A lost claim after
//! the food is already cooking is a real event in a real restaurant, and pretending otherwise just
//! means the software has no way to say so.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::time::Timestamp;

/// How long a ticket stays claimed after its last activity.
///
/// Ten minutes: long enough that a waiter serving another table does not lose their ticket,
/// short enough that a genuinely abandoned one can be picked up within a service. Tuned by
/// observation, not derivation — expect this to change once real floors are watched.
pub const LEASE_IDLE_TIMEOUT_MILLIS: i64 = 10 * 60 * 1_000;

/// A claim on an open ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketLease {
    pub ticket_id: Uuid,
    pub holder: Uuid,
    /// When the claim was made. Part of the tie-break, so it is the device's clock, recorded.
    pub claimed_at: Timestamp,
    /// Last time the holder touched the ticket. Idleness is measured from here, not from the claim.
    pub touched_at: Timestamp,
}

impl TicketLease {
    #[must_use]
    pub const fn new(ticket_id: Uuid, holder: Uuid, at: Timestamp) -> Self {
        Self {
            ticket_id,
            holder,
            claimed_at: at,
            touched_at: at,
        }
    }

    /// Whether the lease has gone idle by `now`.
    #[must_use]
    pub fn is_expired(&self, now: Timestamp) -> bool {
        now.millis()
            .saturating_sub(self.touched_at.millis())
            .saturating_abs()
            >= LEASE_IDLE_TIMEOUT_MILLIS
    }

    /// Record activity, pushing the idle deadline out.
    #[must_use]
    pub const fn touched(mut self, at: Timestamp) -> Self {
        self.touched_at = at;
        self
    }
}

/// Why a device may or may not take a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimVerdict {
    /// Nobody holds it.
    Free,
    /// Already ours — carry on.
    AlreadyHeld,
    /// Held by someone else, but idle long enough to take.
    ///
    /// Takeable, **not** safe: if the holder is offline rather than absent, both devices will
    /// believe they own it until they sync.
    Stale { holder: Uuid },
    /// Held and active. Refuse.
    Held { holder: Uuid },
}

impl ClaimVerdict {
    /// Whether a device may proceed to append to the ticket.
    #[must_use]
    pub const fn permits_claim(self) -> bool {
        matches!(self, Self::Free | Self::AlreadyHeld | Self::Stale { .. })
    }

    /// Whether taking it risks a second device believing the same thing.
    ///
    /// The UI should warn on this, and irreversible actions should hesitate.
    #[must_use]
    pub const fn is_contested(self) -> bool {
        matches!(self, Self::Stale { .. })
    }
}

/// Decide whether `device` may take `ticket`.
#[must_use]
pub fn evaluate_claim(lease: Option<&TicketLease>, device: Uuid, now: Timestamp) -> ClaimVerdict {
    let Some(lease) = lease else {
        return ClaimVerdict::Free;
    };
    if lease.holder == device {
        return ClaimVerdict::AlreadyHeld;
    }
    if lease.is_expired(now) {
        return ClaimVerdict::Stale {
            holder: lease.holder,
        };
    }
    ClaimVerdict::Held {
        holder: lease.holder,
    }
}

/// Pick the winner when two devices claimed the same ticket while apart.
///
/// **Earliest claim wins**, ties broken by device id. Two properties matter more than which rule is
/// chosen:
///
/// - It is *total* — every pair has a winner, so no sync can stall waiting for a human.
/// - It is *deterministic* — both devices and the server compute the same answer from the same two
///   events, so nobody has to ask which copy is authoritative.
///
/// Earliest-wins is preferred over last-writer-wins because the first waiter to claim a table is
/// usually the one standing at it. The device id tie-break is arbitrary, and only reachable when
/// two clocks agree to the millisecond.
#[must_use]
pub fn resolve_contest(first: &TicketLease, second: &TicketLease) -> TicketLease {
    match first.claimed_at.millis().cmp(&second.claimed_at.millis()) {
        core::cmp::Ordering::Less => *first,
        core::cmp::Ordering::Greater => *second,
        core::cmp::Ordering::Equal => {
            if first.holder <= second.holder {
                *first
            } else {
                *second
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    const WAITER_A: u128 = 0xA;
    const WAITER_B: u128 = 0xB;
    const TICKET: u128 = 0x7;

    fn at(minutes: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + minutes * 60 * 1_000)
    }

    fn held_by(device: u128, since: i64) -> TicketLease {
        TicketLease::new(id(TICKET), id(device), at(since))
    }

    #[test]
    fn an_unheld_ticket_is_free() {
        assert_eq!(
            evaluate_claim(None, id(WAITER_A), at(0)),
            ClaimVerdict::Free
        );
    }

    #[test]
    fn the_holder_may_keep_working_on_it() {
        let lease = held_by(WAITER_A, 0);
        assert_eq!(
            evaluate_claim(Some(&lease), id(WAITER_A), at(5)),
            ClaimVerdict::AlreadyHeld
        );
    }

    #[test]
    fn another_waiter_is_refused_while_the_ticket_is_active() {
        // The everyday case: two waiters must not both be adding to one table's order.
        let lease = held_by(WAITER_A, 0);
        assert_eq!(
            evaluate_claim(Some(&lease), id(WAITER_B), at(5)),
            ClaimVerdict::Held {
                holder: id(WAITER_A)
            }
        );
        assert!(!evaluate_claim(Some(&lease), id(WAITER_B), at(5)).permits_claim());
    }

    #[test]
    fn an_idle_ticket_becomes_takeable_but_is_flagged_contested() {
        // Takeable is not the same as safe. If the holder is merely offline, both devices now
        // believe they own it.
        let lease = held_by(WAITER_A, 0);
        let verdict = evaluate_claim(Some(&lease), id(WAITER_B), at(11));

        assert_eq!(
            verdict,
            ClaimVerdict::Stale {
                holder: id(WAITER_A)
            }
        );
        assert!(verdict.permits_claim());
        assert!(
            verdict.is_contested(),
            "the UI must warn before an irreversible action"
        );
    }

    #[test]
    fn activity_pushes_the_idle_deadline_out() {
        // A waiter serving another table for nine minutes must not lose their ticket.
        let lease = held_by(WAITER_A, 0).touched(at(9));
        assert_eq!(
            evaluate_claim(Some(&lease), id(WAITER_B), at(15)),
            ClaimVerdict::Held {
                holder: id(WAITER_A)
            }
        );
    }

    #[test]
    fn expiry_is_measured_from_last_activity_not_from_the_claim() {
        let lease = held_by(WAITER_A, 0).touched(at(30));
        assert!(!lease.is_expired(at(35)));
        assert!(lease.is_expired(at(41)));
    }

    #[test]
    fn a_backwards_clock_does_not_expire_a_live_lease() {
        // Device clocks drift and get corrected. A lease must not evaporate because time moved back.
        let lease = held_by(WAITER_A, 10);
        assert!(!lease.is_expired(at(9)), "one minute 'before' the claim");
    }

    #[test]
    fn the_earlier_claim_wins_a_contest() {
        // The first waiter to claim a table is usually the one standing at it.
        let early = held_by(WAITER_B, 3);
        let late = held_by(WAITER_A, 7);

        assert_eq!(resolve_contest(&early, &late).holder, id(WAITER_B));
        assert_eq!(resolve_contest(&late, &early).holder, id(WAITER_B));
    }

    #[test]
    fn resolution_does_not_depend_on_argument_order() {
        // Both devices and the server run this on the same pair and must agree.
        let a = held_by(WAITER_A, 5);
        let b = held_by(WAITER_B, 5);

        assert_eq!(resolve_contest(&a, &b), resolve_contest(&b, &a));
    }

    #[test]
    fn identical_timestamps_are_broken_deterministically() {
        // Only reachable when two clocks agree to the millisecond, but it must still be total —
        // no sync may stall waiting for a human.
        let a = held_by(WAITER_A, 5);
        let b = held_by(WAITER_B, 5);

        assert_eq!(
            resolve_contest(&a, &b).holder,
            id(WAITER_A),
            "lower id wins"
        );
    }

    #[test]
    fn a_contest_always_has_exactly_one_winner() {
        for first_minute in 0..5 {
            for second_minute in 0..5 {
                let a = held_by(WAITER_A, first_minute);
                let b = held_by(WAITER_B, second_minute);
                let winner = resolve_contest(&a, &b);

                assert!(
                    winner.holder == id(WAITER_A) || winner.holder == id(WAITER_B),
                    "resolution must be total"
                );
            }
        }
    }
}
