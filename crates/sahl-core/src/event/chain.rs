use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::envelope::{EventEnvelope, EventHeader, EventPayload};
use super::error::EventError;
use super::hash::EventHash;

/// Where a device's chain currently ends.
///
/// Persisting this is what lets a terminal resume its chain after a restart without replaying
/// every event it has ever written — which matters when a busy shop's log runs to millions of rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainTip {
    /// Sequence number of the last sealed event. Zero means nothing has been written.
    pub device_seq: u64,
    /// Digest of the last sealed event, or [`EventHash::GENESIS`] if nothing has been written.
    pub hash: EventHash,
}

impl ChainTip {
    /// The starting tip for a device that has never written an event.
    pub const GENESIS: Self = Self {
        device_seq: 0,
        hash: EventHash::GENESIS,
    };
}

impl Default for ChainTip {
    fn default() -> Self {
        Self::GENESIS
    }
}

/// The append-only event chain for a single device.
///
/// One chain per device, never shared. Two terminals in the same shop write two independent chains
/// that the server interleaves on sync — which is what lets both keep selling while the internet is
/// down, with neither needing to coordinate with the other.
///
/// The chain owns only the tip. It is not a store: persistence lives in the terminal and server
/// crates, because `sahl-core` does no I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventChain {
    device_id: Uuid,
    tip: ChainTip,
}

impl EventChain {
    /// Start a fresh chain for a newly enrolled device.
    #[must_use]
    pub const fn new(device_id: Uuid) -> Self {
        Self {
            device_id,
            tip: ChainTip::GENESIS,
        }
    }

    /// Resume an existing chain from its persisted tip.
    #[must_use]
    pub const fn resume(device_id: Uuid, tip: ChainTip) -> Self {
        Self { device_id, tip }
    }

    /// The device this chain belongs to.
    #[must_use]
    pub const fn device_id(&self) -> Uuid {
        self.device_id
    }

    /// The current tip.
    #[must_use]
    pub const fn tip(&self) -> ChainTip {
        self.tip
    }

    /// Seal `payload` onto the end of the chain and advance the tip.
    ///
    /// The sequence increments by exactly one, and the new event links to the previous digest. Both
    /// are what make a deleted event detectable: removing one leaves a sequence gap *and* a broken
    /// link, and forging around it would require recomputing every subsequent digest.
    ///
    /// # Errors
    /// [`EventError::DeviceMismatch`] if the header names a different device,
    /// [`EventError::SequenceBreak`] if the sequence would overflow, or
    /// [`EventError::NotCanonical`] if the payload cannot be canonically hashed.
    pub fn append<P: EventPayload>(
        &mut self,
        header: EventHeader,
        payload: &P,
    ) -> Result<EventEnvelope, EventError> {
        if header.device_id != self.device_id {
            return Err(EventError::DeviceMismatch {
                expected: self.device_id,
                found: header.device_id,
            });
        }

        let device_seq = self
            .tip
            .device_seq
            .checked_add(1)
            .ok_or(EventError::SequenceBreak {
                previous: self.tip.device_seq,
                expected: self.tip.device_seq,
                found: self.tip.device_seq,
            })?;

        let event = EventEnvelope::seal(header, device_seq, payload, self.tip.hash)?;

        self.tip = ChainTip {
            device_seq,
            hash: event.hash,
        };
        Ok(event)
    }
}

/// Verify a run of events forms an unbroken chain starting from `start`.
///
/// This is the check the server runs on every sync batch, and it is the reason the event log is
/// worth more than a table of sales. It proves four things at once:
///
/// - **No event was altered** — each digest is recomputed from contents.
/// - **No event was removed** — a gap breaks both the sequence and the hash link.
/// - **No event was inserted** — an inserted event's successor would no longer link correctly.
/// - **No two chains were spliced** — every event must name the same device.
///
/// Passing an empty slice is not an error; it verifies trivially and returns `start`.
///
/// # Errors
/// The specific [`EventError`] describing the first inconsistency found.
pub fn verify_chain(events: &[EventEnvelope], start: ChainTip) -> Result<ChainTip, EventError> {
    let Some(first) = events.first() else {
        return Ok(start);
    };

    let device_id = first.device_id;
    let mut tip = start;

    for event in events {
        if event.device_id != device_id {
            return Err(EventError::DeviceMismatch {
                expected: device_id,
                found: event.device_id,
            });
        }

        let expected_seq = tip
            .device_seq
            .checked_add(1)
            .ok_or(EventError::SequenceBreak {
                previous: tip.device_seq,
                expected: tip.device_seq,
                found: event.device_seq,
            })?;
        if event.device_seq != expected_seq {
            return Err(EventError::SequenceBreak {
                previous: tip.device_seq,
                expected: expected_seq,
                found: event.device_seq,
            });
        }

        if event.prev_hash != tip.hash {
            return Err(EventError::BrokenLink {
                event_id: event.event_id,
                expected: event.prev_hash,
                actual: tip.hash,
            });
        }

        event.verify()?;

        tip = ChainTip {
            device_seq: event.device_seq,
            hash: event.hash,
        };
    }

    Ok(tip)
}

/// Verify a chain that must begin at the very first event a device ever wrote.
///
/// # Errors
/// [`EventError::NotGenesis`] if the run does not start at sequence 1 from the genesis hash, plus
/// anything [`verify_chain`] can return.
pub fn verify_chain_from_genesis(events: &[EventEnvelope]) -> Result<ChainTip, EventError> {
    if let Some(first) = events.first()
        && (first.device_seq != 1 || !first.prev_hash.is_genesis())
    {
        return Err(EventError::NotGenesis {
            found: first.device_seq,
            previous: first.prev_hash,
        });
    }
    verify_chain(events, ChainTip::GENESIS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Timestamp;
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

    fn uuid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    const DEVICE: u128 = 4;

    fn header(n: u32) -> EventHeader {
        EventHeader {
            event_id: uuid(1_000 + u128::from(n)),
            tenant_id: uuid(2),
            outlet_id: uuid(3),
            device_id: uuid(DEVICE),
            occurred_at: Timestamp::from_millis(1_753_000_000_000 + i64::from(n)),
        }
    }

    fn chain_of(count: u32) -> (EventChain, Vec<EventEnvelope>) {
        let mut chain = EventChain::new(uuid(DEVICE));
        let events = (0..count)
            .map(|n| chain.append(header(n), &Tick { n }).expect("appends"))
            .collect();
        (chain, events)
    }

    #[test]
    fn a_fresh_chain_starts_at_genesis() {
        let chain = EventChain::new(uuid(DEVICE));
        assert_eq!(chain.tip(), ChainTip::GENESIS);
        assert_eq!(chain.tip().device_seq, 0);
    }

    #[test]
    fn appending_increments_the_sequence_and_links_the_hash() {
        let (_, events) = chain_of(3);

        assert_eq!(
            events.iter().map(|e| e.device_seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(events[0].prev_hash.is_genesis());
        assert_eq!(events[1].prev_hash, events[0].hash);
        assert_eq!(events[2].prev_hash, events[1].hash);
    }

    #[test]
    fn a_well_formed_chain_verifies() {
        let (chain, events) = chain_of(10);
        assert_eq!(verify_chain_from_genesis(&events), Ok(chain.tip()));
    }

    #[test]
    fn an_empty_batch_verifies_trivially() {
        assert_eq!(verify_chain(&[], ChainTip::GENESIS), Ok(ChainTip::GENESIS));
    }

    #[test]
    fn a_chain_resumes_from_a_persisted_tip() {
        // A terminal restarting mid-shift must not have to replay its whole log.
        let (original, events) = chain_of(5);
        let mut resumed = EventChain::resume(uuid(DEVICE), original.tip());
        let next = resumed
            .append(header(99), &Tick { n: 99 })
            .expect("appends");

        assert_eq!(next.device_seq, 6);
        assert_eq!(next.prev_hash, events[4].hash);
        assert_eq!(
            verify_chain(&[next], original.tip()).map(|t| t.device_seq),
            Ok(6)
        );
    }

    #[test]
    fn a_deleted_event_is_caught() {
        // The headline case: a cashier voids a sale by removing its row from SQLite.
        let (_, mut events) = chain_of(5);
        events.remove(2);

        assert!(matches!(
            verify_chain_from_genesis(&events),
            Err(EventError::SequenceBreak { .. })
        ));
    }

    #[test]
    fn a_truncated_tail_is_caught_by_the_expected_tip() {
        // Deleting from the *end* leaves the remaining chain internally valid, which is exactly why
        // the server compares the resulting tip against what the device claims.
        let (chain, mut events) = chain_of(5);
        events.truncate(3);

        let verified = verify_chain_from_genesis(&events).expect("prefix is internally valid");
        assert_ne!(
            verified,
            chain.tip(),
            "a truncated log must not match the claimed tip"
        );
        assert_eq!(verified.device_seq, 3);
    }

    #[test]
    fn an_altered_event_is_caught() {
        let (_, mut events) = chain_of(5);
        events[2].payload = serde_json::json!({ "n": 999 });

        assert!(matches!(
            verify_chain_from_genesis(&events),
            Err(EventError::HashMismatch { .. })
        ));
    }

    #[test]
    fn reordering_events_is_caught() {
        let (_, mut events) = chain_of(5);
        events.swap(1, 3);

        assert!(matches!(
            verify_chain_from_genesis(&events),
            Err(EventError::SequenceBreak { .. })
        ));
    }

    #[test]
    fn splicing_in_an_event_from_another_device_is_caught() {
        let (_, mut events) = chain_of(3);
        let mut other = EventChain::new(uuid(77));
        let mut foreign_header = header(0);
        foreign_header.device_id = uuid(77);
        let foreign = other
            .append(foreign_header, &Tick { n: 0 })
            .expect("appends");
        events.push(foreign);

        assert!(matches!(
            verify_chain_from_genesis(&events),
            Err(EventError::DeviceMismatch { .. })
        ));
    }

    #[test]
    fn a_relinked_event_is_caught_even_if_resealed() {
        // Rewriting an event's own hash to match its tampered contents still leaves its successor
        // pointing at the old digest.
        let (_, mut events) = chain_of(4);
        events[1].payload = serde_json::json!({ "n": 42 });
        events[1].hash = events[1].recompute_hash().expect("recomputes");

        assert_eq!(
            events[1].verify(),
            Ok(()),
            "the forged event verifies alone"
        );
        assert!(
            matches!(
                verify_chain_from_genesis(&events),
                Err(EventError::BrokenLink { .. })
            ),
            "but the chain does not"
        );
    }

    #[test]
    fn a_batch_that_does_not_start_at_genesis_is_refused() {
        let (_, events) = chain_of(5);
        assert!(matches!(
            verify_chain_from_genesis(&events[2..]),
            Err(EventError::NotGenesis { .. })
        ));
    }

    #[test]
    fn a_batch_starting_mid_chain_verifies_against_the_right_tip() {
        let (_, events) = chain_of(5);
        let tip_after_two = ChainTip {
            device_seq: 2,
            hash: events[1].hash,
        };

        assert!(verify_chain(&events[2..], tip_after_two).is_ok());
        assert!(verify_chain(&events[2..], ChainTip::GENESIS).is_err());
    }

    #[test]
    fn appending_with_a_foreign_device_header_is_refused() {
        let mut chain = EventChain::new(uuid(DEVICE));
        let mut foreign = header(0);
        foreign.device_id = uuid(99);

        assert!(matches!(
            chain.append(foreign, &Tick { n: 0 }),
            Err(EventError::DeviceMismatch { .. })
        ));
    }
}
