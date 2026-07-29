//! Deciding what to do with a pushed batch.
//!
//! Pure and separate from the database on purpose: this is the logic that decides whether a
//! merchant's sales are accepted, and it should be testable without a server running.

use sahl_core::event::{ChainTip, EventEnvelope, verify_chain};
use uuid::Uuid;

use crate::error::SyncError;

/// What the server should do with a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchPlan {
    /// Leading events already stored — a retry. Verified against the tip, then skipped.
    pub already_stored: usize,
    /// Events to insert, starting at index `already_stored`.
    pub to_insert: usize,
    /// Where the chain will end once they are committed.
    pub resulting_tip: ChainTip,
}

impl BatchPlan {
    /// Nothing new — the whole batch was already accepted.
    #[must_use]
    pub const fn is_noop(&self) -> bool {
        self.to_insert == 0
    }
}

/// Decide how to apply `events` to a device whose chain currently ends at `tip`.
///
/// The three cases that matter, and why each is handled the way it is:
///
/// - **A gap.** The batch starts beyond the tip, so events are missing. Rejected — accepting it
///   would leave a hole no later push can fill, since sequences only go forward.
/// - **A retry.** The response to an earlier push was lost, so the terminal sends the same events
///   again. Accepted as a no-op rather than an error, because a network that drops one response
///   will drop others, and a till that cannot retry safely stops selling.
/// - **A partial retry.** Some events are already stored and some are new. The overlap is verified
///   by hash and skipped; the remainder is verified and inserted.
///
/// # Errors
/// [`SyncError`] describing the first inconsistency found.
pub fn plan_batch(
    events: &[EventEnvelope],
    tip: ChainTip,
    device_id: Uuid,
) -> Result<BatchPlan, SyncError> {
    let Some(first) = events.first() else {
        return Ok(BatchPlan {
            already_stored: 0,
            to_insert: 0,
            resulting_tip: tip,
        });
    };

    for event in events {
        if event.device_id != device_id {
            return Err(SyncError::WrongDevice {
                expected: device_id,
                found: event.device_id,
            });
        }
    }

    // Sequences only move forward, so a hole here can never be filled later.
    if first.device_seq > tip.device_seq.saturating_add(1) {
        return Err(SyncError::Gap {
            tip: tip.device_seq,
            batch_starts_at: first.device_seq,
        });
    }

    // How many leading events the server has already seen.
    let overlap = tip
        .device_seq
        .saturating_add(1)
        .saturating_sub(first.device_seq);
    let already_stored = usize::try_from(overlap)
        .unwrap_or(usize::MAX)
        .min(events.len());

    let fresh = events.get(already_stored..).unwrap_or(&[]);
    if fresh.is_empty() {
        return Ok(BatchPlan {
            already_stored,
            to_insert: 0,
            resulting_tip: tip,
        });
    }

    // The last already-stored event must hash to the stored tip. If it does not, the terminal is
    // replaying a *different* history than the one the server accepted — a fork, not a retry.
    if already_stored > 0 {
        let boundary = events
            .get(already_stored.saturating_sub(1))
            .ok_or(SyncError::Gap {
                tip: tip.device_seq,
                batch_starts_at: first.device_seq,
            })?;
        if boundary.hash != tip.hash {
            return Err(SyncError::Forked {
                device_seq: boundary.device_seq,
                server_hash: tip.hash,
                device_hash: boundary.hash,
            });
        }
    }

    let resulting_tip = verify_chain(fresh, tip)?;

    Ok(BatchPlan {
        already_stored,
        to_insert: fresh.len(),
        resulting_tip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sahl_core::Timestamp;
    use sahl_core::event::{EventChain, EventHash, EventHeader, EventPayload};
    use serde::Serialize;

    #[derive(Serialize)]
    struct Tick {
        n: u32,
    }
    impl EventPayload for Tick {
        fn kind(&self) -> &'static str {
            "test.tick"
        }
    }

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    const DEVICE: u128 = 0xD3;

    fn build(count: u32) -> Vec<EventEnvelope> {
        let mut chain = EventChain::new(id(DEVICE));
        (0..count)
            .map(|n| {
                chain
                    .append(
                        EventHeader {
                            event_id: id(1_000 + u128::from(n)),
                            tenant_id: id(2),
                            outlet_id: id(3),
                            device_id: id(DEVICE),
                            occurred_at: Timestamp::from_millis(1_753_000_000_000 + i64::from(n)),
                        },
                        &Tick { n },
                    )
                    .expect("appends")
            })
            .collect()
    }

    fn tip_after(events: &[EventEnvelope], count: usize) -> ChainTip {
        if count == 0 {
            return ChainTip::GENESIS;
        }
        let last = &events[count - 1];
        ChainTip {
            device_seq: last.device_seq,
            hash: last.hash,
        }
    }

    #[test]
    fn a_fresh_device_accepts_everything() {
        let events = build(5);
        let plan = plan_batch(&events, ChainTip::GENESIS, id(DEVICE)).expect("plans");

        assert_eq!(plan.already_stored, 0);
        assert_eq!(plan.to_insert, 5);
        assert_eq!(plan.resulting_tip.device_seq, 5);
    }

    #[test]
    fn an_empty_batch_is_a_noop() {
        let plan = plan_batch(&[], ChainTip::GENESIS, id(DEVICE)).expect("plans");
        assert!(plan.is_noop());
    }

    #[test]
    fn a_full_retry_is_a_noop_not_an_error() {
        // The response to the first push was lost. A till that cannot retry safely stops selling.
        let events = build(5);
        let tip = tip_after(&events, 5);
        let plan = plan_batch(&events, tip, id(DEVICE)).expect("plans");

        assert!(plan.is_noop());
        assert_eq!(plan.already_stored, 5);
        assert_eq!(plan.resulting_tip, tip);
    }

    #[test]
    fn a_partial_retry_skips_the_overlap_and_takes_the_rest() {
        // The terminal kept selling while the ack was in flight, so it resends 1-5 plus 6-8.
        let events = build(8);
        let tip = tip_after(&events, 5);
        let plan = plan_batch(&events, tip, id(DEVICE)).expect("plans");

        assert_eq!(plan.already_stored, 5);
        assert_eq!(plan.to_insert, 3);
        assert_eq!(plan.resulting_tip.device_seq, 8);
    }

    #[test]
    fn a_gap_is_refused() {
        // Sequences only go forward, so a hole accepted now can never be filled.
        let events = build(10);
        let plan = plan_batch(&events[5..], ChainTip::GENESIS, id(DEVICE));

        assert_eq!(
            plan,
            Err(SyncError::Gap {
                tip: 0,
                batch_starts_at: 6
            })
        );
    }

    #[test]
    fn a_fork_is_refused_rather_than_treated_as_a_retry() {
        // Same sequence numbers, different history — a restored backup, or tampering. Either way
        // the server must not silently continue on a chain it never agreed to.
        let ours = build(5);
        let tip = tip_after(&ours, 5);

        let mut theirs = build(8);
        theirs[4].hash = EventHash::digest(b"different history");

        let plan = plan_batch(&theirs, tip, id(DEVICE));
        assert!(matches!(plan, Err(SyncError::Forked { .. })));
    }

    #[test]
    fn a_batch_from_another_device_is_refused() {
        let mut events = build(3);
        events[1].device_id = id(0xBEEF);

        assert!(matches!(
            plan_batch(&events, ChainTip::GENESIS, id(DEVICE)),
            Err(SyncError::WrongDevice { .. })
        ));
    }

    #[test]
    fn a_tampered_event_inside_the_new_portion_is_refused() {
        let mut events = build(5);
        events[3].payload = serde_json::json!({ "n": 999 });

        assert!(plan_batch(&events, ChainTip::GENESIS, id(DEVICE)).is_err());
    }

    #[test]
    fn planning_the_same_batch_twice_gives_the_same_plan() {
        let events = build(6);
        let tip = tip_after(&events, 2);

        assert_eq!(
            plan_batch(&events, tip, id(DEVICE)),
            plan_batch(&events, tip, id(DEVICE))
        );
    }

    #[test]
    fn a_batch_arriving_exactly_at_the_tip_boundary_is_all_new() {
        let events = build(10);
        let tip = tip_after(&events, 4);
        let plan = plan_batch(&events[4..], tip, id(DEVICE)).expect("plans");

        assert_eq!(plan.already_stored, 0);
        assert_eq!(plan.to_insert, 6);
    }
}
