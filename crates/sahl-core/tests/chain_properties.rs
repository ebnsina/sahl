//! Chain-integrity fuzzing.
//!
//! The plan commits to this explicitly: mutate, reorder, and drop events, and assert the verifier
//! rejects every case. These properties are what let the product claim its event log is *evidence*
//! rather than merely a record — the basis of both the fraud-detection wedge and, later, ZATCA's
//! mandated invoice chain.
//!
//! One nuance the properties encode carefully: removing events from the **end** of a chain leaves a
//! prefix that is still internally consistent. No hash check can catch that, and pretending
//! otherwise would be a false claim. What catches it is comparing the verified tip against the tip
//! the device claims — so that is asserted separately.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use proptest::prelude::*;
use sahl_core::Timestamp;
use sahl_core::event::{
    ChainTip, EventChain, EventEnvelope, EventError, EventHeader, EventPayload,
    verify_chain_from_genesis,
};
use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize)]
struct Tick {
    n: u32,
    note: String,
}

impl EventPayload for Tick {
    fn kind(&self) -> &'static str {
        "test.tick"
    }
}

const DEVICE: u128 = 0xD3;

fn uuid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// Build a well-formed chain of `count` events.
fn build(count: u32) -> (ChainTip, Vec<EventEnvelope>) {
    let mut chain = EventChain::new(uuid(DEVICE));
    let events = (0..count)
        .map(|n| {
            let header = EventHeader {
                event_id: uuid(1_000 + u128::from(n)),
                tenant_id: uuid(2),
                outlet_id: uuid(3),
                device_id: uuid(DEVICE),
                occurred_at: Timestamp::from_millis(1_753_000_000_000 + i64::from(n)),
            };
            chain
                .append(
                    header,
                    &Tick {
                        n,
                        note: format!("event {n}"),
                    },
                )
                .expect("appends")
        })
        .collect();
    (chain.tip(), events)
}

proptest! {
    /// Any chain built through the normal path verifies, at any length.
    #[test]
    fn a_well_formed_chain_always_verifies(count in 1u32..=60) {
        let (tip, events) = build(count);
        prop_assert_eq!(verify_chain_from_genesis(&events).expect("verifies"), tip);
    }

    /// Every event is uniquely identified by its digest — no two events in a chain collide, even
    /// though their payloads differ only slightly.
    #[test]
    fn every_event_in_a_chain_has_a_distinct_hash(count in 2u32..=60) {
        let (_, events) = build(count);
        let mut hashes: Vec<_> = events.iter().map(|event| event.hash).collect();
        let total = hashes.len();
        hashes.sort_unstable();
        hashes.dedup();

        prop_assert_eq!(hashes.len(), total);
    }

    /// **Dropping an event from the middle is always caught.** This is the headline case: a cashier
    /// deleting the row for a sale they pocketed.
    #[test]
    fn removing_any_interior_event_is_caught(count in 2u32..=40, seed: u64) {
        let (_, mut events) = build(count);
        // Any index except the last — removing the tail is a separate, weaker case.
        let index = usize::try_from(seed % u64::from(count - 1)).unwrap();
        events.remove(index);

        prop_assert!(
            verify_chain_from_genesis(&events).is_err(),
            "removing event {index} of {count} went undetected"
        );
    }

    /// Removing events from the end cannot be caught by hashing alone — the remainder is a valid
    /// prefix. What catches it is the tip: a truncated log verifies to a *different* tip than the
    /// device claims, which is the check the server performs on sync.
    #[test]
    fn truncating_the_tail_is_caught_by_the_tip_not_the_hashes(
        count in 2u32..=40,
        drop_count in 1u32..=10,
    ) {
        let drop_count = drop_count.min(count - 1);
        let (claimed_tip, mut events) = build(count);
        events.truncate(usize::try_from(count - drop_count).unwrap());

        let verified = verify_chain_from_genesis(&events).expect("a prefix stays internally valid");
        prop_assert_ne!(verified, claimed_tip, "a truncated log must not match the claimed tip");
        prop_assert_eq!(verified.device_seq, u64::from(count - drop_count));
    }

    /// Reordering is always caught, because the sequence must increment by exactly one.
    #[test]
    fn swapping_any_two_events_is_caught(count in 2u32..=40, a: u64, b: u64) {
        let (_, mut events) = build(count);
        let left = usize::try_from(a % u64::from(count)).unwrap();
        let right = usize::try_from(b % u64::from(count)).unwrap();
        prop_assume!(left != right);
        events.swap(left, right);

        prop_assert!(
            verify_chain_from_genesis(&events).is_err(),
            "swapping {left} and {right} went undetected"
        );
    }

    /// Altering any event's payload is caught by its own digest.
    #[test]
    fn altering_any_payload_is_caught(count in 1u32..=40, seed: u64, new_value: u32) {
        let (_, mut events) = build(count);
        let index = usize::try_from(seed % u64::from(count)).unwrap();
        let original = events[index].payload.clone();
        events[index].payload = serde_json::json!({ "n": new_value, "note": "tampered" });
        prop_assume!(events[index].payload != original);

        let caught = matches!(
            verify_chain_from_genesis(&events),
            Err(EventError::HashMismatch { .. })
        );
        prop_assert!(caught, "altering payload of event {index} went undetected");
    }

    /// Altering a timestamp is caught. Backdating a sale to a previous shift is a real fraud
    /// pattern, and it changes the digest.
    #[test]
    fn altering_any_timestamp_is_caught(count in 1u32..=40, seed: u64, millis: i64) {
        let (_, mut events) = build(count);
        let index = usize::try_from(seed % u64::from(count)).unwrap();
        prop_assume!(events[index].occurred_at.millis() != millis);
        events[index].occurred_at = Timestamp::from_millis(millis);

        let caught = matches!(
            verify_chain_from_genesis(&events),
            Err(EventError::HashMismatch { .. })
        );
        prop_assert!(caught, "backdating event {index} went undetected");
    }

    /// Re-sealing a tampered event so it verifies in isolation still breaks the chain, because its
    /// successor embeds the *old* digest. Forging a single event is not enough — an attacker would
    /// have to rewrite every event after it.
    #[test]
    fn resealing_a_tampered_event_still_breaks_the_chain(count in 2u32..=40, seed: u64) {
        let (_, mut events) = build(count);
        // Anything but the last event, which has no successor to contradict it.
        let index = usize::try_from(seed % u64::from(count - 1)).unwrap();

        events[index].payload = serde_json::json!({ "n": 999_999, "note": "forged" });
        events[index].hash = events[index].recompute_hash().expect("recomputes");

        prop_assert_eq!(events[index].verify(), Ok(()), "the forged event verifies alone");
        prop_assert!(
            matches!(
                verify_chain_from_genesis(&events),
                Err(EventError::BrokenLink { .. })
            ),
            "but its successor must still contradict it"
        );
    }

    /// Splicing in an event from a different device is caught. Each device owns exactly one chain.
    #[test]
    fn splicing_a_foreign_device_is_caught(count in 1u32..=30, foreign_device in 1u128..=1_000) {
        prop_assume!(foreign_device != DEVICE);
        let (_, mut events) = build(count);
        let mut foreign = events[events.len() - 1].clone();
        foreign.device_id = uuid(foreign_device);
        events.push(foreign);

        let caught = matches!(
            verify_chain_from_genesis(&events),
            Err(EventError::DeviceMismatch { .. })
        );
        prop_assert!(caught, "an event from device {foreign_device} was accepted");
    }

    /// A batch that does not begin at the device's very first event is refused by the
    /// genesis-anchored check.
    #[test]
    fn a_chain_not_starting_at_genesis_is_refused(count in 2u32..=30, skip in 1u32..=5) {
        let skip = usize::try_from(skip.min(count - 1)).unwrap();
        let (_, events) = build(count);

        let caught = matches!(
            verify_chain_from_genesis(&events[skip..]),
            Err(EventError::NotGenesis { .. })
        );
        prop_assert!(caught, "a batch skipping {skip} events was accepted as genesis");
    }

    /// Corrupting a stored digest directly is caught.
    #[test]
    fn flipping_a_byte_of_any_stored_hash_is_caught(count in 1u32..=30, seed: u64, byte in 0usize..32) {
        let (_, mut events) = build(count);
        let index = usize::try_from(seed % u64::from(count)).unwrap();

        let mut bytes = *events[index].hash.as_bytes();
        bytes[byte] ^= 0xFF;
        events[index].hash = sahl_core::EventHash::from_bytes(bytes);

        prop_assert!(verify_chain_from_genesis(&events).is_err());
    }

    /// Building the same chain twice produces byte-identical events. The terminal and the server
    /// both derive digests; any divergence here would break every sync.
    #[test]
    fn chain_construction_is_deterministic(count in 1u32..=40) {
        let (first_tip, first) = build(count);
        let (second_tip, second) = build(count);

        prop_assert_eq!(first, second);
        prop_assert_eq!(first_tip, second_tip);
    }
}
