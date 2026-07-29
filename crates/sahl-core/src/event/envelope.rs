use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::time::Timestamp;

use super::canonical::canonical_bytes;
use super::error::EventError;
use super::hash::EventHash;

/// A domain event that can be written to the log.
///
/// Implementors supply a stable `kind` string. **That string is part of the hash**, so renaming a
/// variant after events exist in the field invalidates every chain containing it. Treat these
/// names as a wire format, not as a label.
pub trait EventPayload: Serialize {
    /// Stable discriminator, e.g. `"sale.completed"`.
    fn kind(&self) -> &'static str;
}

/// Everything about an event except its sequence, payload, and chain linkage.
///
/// Grouped into a struct because these five values travel together everywhere, and because it puts
/// the clock and the ID generator in the *caller's* hands. `sahl-core` never reads a clock: that is
/// what makes sealing a pure function, and therefore replayable in tests and identical on the
/// terminal and the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventHeader {
    /// Globally unique, and the idempotency key for sync. UUID v7 so it sorts by creation time.
    pub event_id: Uuid,
    /// The merchant this event belongs to.
    pub tenant_id: Uuid,
    /// The shop or branch.
    pub outlet_id: Uuid,
    /// The terminal that produced it. Each device owns exactly one chain.
    pub device_id: Uuid,
    /// When the terminal believes it happened. Device clocks drift; the server records its own
    /// receipt time separately rather than overwriting this.
    pub occurred_at: Timestamp,
}

/// A sealed event: immutable, hashed, and linked to its predecessor.
///
/// Sealed means `hash` was computed over every other field including `prev_hash`. Change anything
/// and the digest no longer matches — which is what [`EventEnvelope::verify`] checks, and what
/// makes an altered log detectable rather than merely unlikely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_id: Uuid,
    pub tenant_id: Uuid,
    pub outlet_id: Uuid,
    pub device_id: Uuid,
    /// Monotonic per device, starting at 1. A gap means events were lost or removed.
    pub device_seq: u64,
    pub occurred_at: Timestamp,
    /// The payload's stable discriminator.
    pub kind: String,
    /// The payload, canonicalised.
    pub payload: Value,
    /// Digest of the preceding event, or [`EventHash::GENESIS`] for the first.
    pub prev_hash: EventHash,
    /// Digest of every field above.
    pub hash: EventHash,
}

/// The exact field set an event is hashed over — everything but the hash itself.
///
/// A separate struct rather than skip-serializing on the envelope, so that what is hashed is stated
/// explicitly in one place. Field order here is irrelevant: canonical serialization sorts keys.
#[derive(Serialize)]
struct HashInput<'a> {
    event_id: Uuid,
    tenant_id: Uuid,
    outlet_id: Uuid,
    device_id: Uuid,
    device_seq: u64,
    occurred_at: Timestamp,
    kind: &'a str,
    payload: &'a Value,
    prev_hash: &'a EventHash,
}

impl EventEnvelope {
    /// Seal a payload into an immutable, hash-linked event.
    ///
    /// # Errors
    /// [`EventError::NotCanonical`] if the payload cannot be canonically serialized — most likely
    /// because it contains a floating-point number.
    pub fn seal<P: EventPayload>(
        header: EventHeader,
        device_seq: u64,
        payload: &P,
        prev_hash: EventHash,
    ) -> Result<Self, EventError> {
        let kind = payload.kind();
        let payload_value =
            serde_json::to_value(payload).map_err(|source| EventError::NotCanonical {
                reason: source.to_string(),
            })?;

        let hash = Self::compute_hash(&header, device_seq, kind, &payload_value, &prev_hash)?;

        Ok(Self {
            event_id: header.event_id,
            tenant_id: header.tenant_id,
            outlet_id: header.outlet_id,
            device_id: header.device_id,
            device_seq,
            occurred_at: header.occurred_at,
            kind: kind.to_owned(),
            payload: payload_value,
            prev_hash,
            hash,
        })
    }

    /// Recompute this event's digest from its own contents.
    ///
    /// # Errors
    /// [`EventError::NotCanonical`] if the stored payload is not canonically serialisable.
    pub fn recompute_hash(&self) -> Result<EventHash, EventError> {
        let header = EventHeader {
            event_id: self.event_id,
            tenant_id: self.tenant_id,
            outlet_id: self.outlet_id,
            device_id: self.device_id,
            occurred_at: self.occurred_at,
        };
        Self::compute_hash(
            &header,
            self.device_seq,
            &self.kind,
            &self.payload,
            &self.prev_hash,
        )
    }

    /// Verify that this event's contents still match its digest.
    ///
    /// # Errors
    /// [`EventError::HashMismatch`] if the event has been altered since sealing.
    pub fn verify(&self) -> Result<(), EventError> {
        let computed = self.recompute_hash()?;
        if computed == self.hash {
            Ok(())
        } else {
            Err(EventError::HashMismatch {
                event_id: self.event_id,
                stored: self.hash,
                computed,
            })
        }
    }

    /// Deserialize the payload back into a concrete type.
    ///
    /// # Errors
    /// [`EventError::NotCanonical`] if the stored payload does not match `P`'s shape.
    pub fn payload_as<P: for<'de> Deserialize<'de>>(&self) -> Result<P, EventError> {
        serde_json::from_value(self.payload.clone()).map_err(|source| EventError::NotCanonical {
            reason: source.to_string(),
        })
    }

    fn compute_hash(
        header: &EventHeader,
        device_seq: u64,
        kind: &str,
        payload: &Value,
        prev_hash: &EventHash,
    ) -> Result<EventHash, EventError> {
        let input = HashInput {
            event_id: header.event_id,
            tenant_id: header.tenant_id,
            outlet_id: header.outlet_id,
            device_id: header.device_id,
            device_seq,
            occurred_at: header.occurred_at,
            kind,
            payload,
            prev_hash,
        };
        Ok(EventHash::digest(&canonical_bytes(&input)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
    struct SaleCompleted {
        sale_id: u32,
        total_minor: i64,
    }

    impl EventPayload for SaleCompleted {
        fn kind(&self) -> &'static str {
            "sale.completed"
        }
    }

    fn uuid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn header() -> EventHeader {
        EventHeader {
            event_id: uuid(1),
            tenant_id: uuid(2),
            outlet_id: uuid(3),
            device_id: uuid(4),
            occurred_at: Timestamp::from_millis(1_753_000_000_000),
        }
    }

    fn sale() -> SaleCompleted {
        SaleCompleted {
            sale_id: 7,
            total_minor: 10_000,
        }
    }

    #[test]
    fn a_sealed_event_verifies_against_itself() {
        let event = EventEnvelope::seal(header(), 1, &sale(), EventHash::GENESIS).expect("seals");
        assert_eq!(event.verify(), Ok(()));
        assert_eq!(event.kind, "sale.completed");
        assert_eq!(event.device_seq, 1);
    }

    #[test]
    fn sealing_is_deterministic() {
        // Two processes sealing the same event must derive the same digest, or sync breaks.
        let first = EventEnvelope::seal(header(), 1, &sale(), EventHash::GENESIS).expect("seals");
        let second = EventEnvelope::seal(header(), 1, &sale(), EventHash::GENESIS).expect("seals");
        assert_eq!(first, second);
    }

    #[test]
    fn altering_the_payload_is_detected() {
        let mut event =
            EventEnvelope::seal(header(), 1, &sale(), EventHash::GENESIS).expect("seals");
        event.payload = serde_json::json!({ "sale_id": 7, "total_minor": 1 });

        assert!(matches!(
            event.verify(),
            Err(EventError::HashMismatch { .. })
        ));
    }

    #[test]
    fn altering_the_sequence_is_detected() {
        // Renumbering events to close a gap left by a deleted sale must not go unnoticed.
        let mut event =
            EventEnvelope::seal(header(), 5, &sale(), EventHash::GENESIS).expect("seals");
        event.device_seq = 4;

        assert!(matches!(
            event.verify(),
            Err(EventError::HashMismatch { .. })
        ));
    }

    #[test]
    fn altering_the_timestamp_is_detected() {
        let mut event =
            EventEnvelope::seal(header(), 1, &sale(), EventHash::GENESIS).expect("seals");
        event.occurred_at = Timestamp::from_millis(0);

        assert!(matches!(
            event.verify(),
            Err(EventError::HashMismatch { .. })
        ));
    }

    #[test]
    fn relinking_to_a_different_predecessor_is_detected() {
        let mut event =
            EventEnvelope::seal(header(), 1, &sale(), EventHash::GENESIS).expect("seals");
        event.prev_hash = EventHash::digest(b"some other event");

        assert!(matches!(
            event.verify(),
            Err(EventError::HashMismatch { .. })
        ));
    }

    #[test]
    fn the_previous_hash_actually_participates_in_the_digest() {
        let genesis_linked =
            EventEnvelope::seal(header(), 1, &sale(), EventHash::GENESIS).expect("seals");
        let other_linked =
            EventEnvelope::seal(header(), 1, &sale(), EventHash::digest(b"other")).expect("seals");

        assert_ne!(genesis_linked.hash, other_linked.hash);
    }

    #[test]
    fn payload_round_trips_back_to_its_type() {
        let event = EventEnvelope::seal(header(), 1, &sale(), EventHash::GENESIS).expect("seals");
        assert_eq!(event.payload_as::<SaleCompleted>(), Ok(sale()));
    }

    #[test]
    fn a_float_in_a_payload_is_refused_at_the_boundary() {
        #[derive(Serialize)]
        struct Sloppy {
            weight: f64,
        }
        impl EventPayload for Sloppy {
            fn kind(&self) -> &'static str {
                "sloppy"
            }
        }

        let result = EventEnvelope::seal(header(), 1, &Sloppy { weight: 1.5 }, EventHash::GENESIS);
        assert!(matches!(result, Err(EventError::NotCanonical { .. })));
    }
}
