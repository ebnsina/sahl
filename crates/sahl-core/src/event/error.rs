use thiserror::Error;

use uuid::Uuid;

use super::hash::EventHash;

/// Why an event could not be sealed, or why a chain failed to verify.
///
/// Every variant here means "this log has been tampered with, corrupted, or lost data". None of
/// them are recoverable at the till — they are grounds for refusing a sync batch and alerting.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EventError {
    /// A value could not be reduced to canonical bytes.
    #[error("value is not canonically hashable: {reason}")]
    NotCanonical { reason: String },

    /// A hex string was not a valid 32-byte digest.
    #[error("malformed hash: {value}")]
    MalformedHash { value: String },

    /// The recomputed digest does not match the one stored on the event. The event's contents were
    /// altered after it was sealed.
    #[error(
        "event {event_id} has been altered: stored hash {stored} but content hashes to {computed}"
    )]
    HashMismatch {
        event_id: Uuid,
        stored: EventHash,
        computed: EventHash,
    },

    /// An event does not link to the one before it. Either an event was removed, or two chains
    /// were spliced together.
    #[error("event {event_id} links to {expected} but the previous event hashes to {actual}")]
    BrokenLink {
        event_id: Uuid,
        expected: EventHash,
        actual: EventHash,
    },

    /// The per-device sequence skipped, repeated, or went backwards. A gap means events were lost
    /// or deliberately removed — the single most important signal the server checks on sync.
    #[error("device sequence jumped from {previous} to {found} (expected {expected})")]
    SequenceBreak {
        previous: u64,
        expected: u64,
        found: u64,
    },

    /// Events from more than one device appeared in a single chain. Each device owns its own chain.
    #[error("chain contains events from two devices: {expected} and {found}")]
    DeviceMismatch { expected: Uuid, found: Uuid },

    /// A chain that should start at genesis did not.
    #[error("chain starts at sequence {found} with previous hash {previous}, not at genesis")]
    NotGenesis { found: u64, previous: EventHash },

    /// Time moved backwards within a device's chain. Not fatal on its own — clocks drift and are
    /// corrected — but it is recorded because it is a fraud signal worth surfacing to an owner.
    #[error("event {event_id} occurred at {found}ms, before its predecessor at {previous}ms")]
    TimeWentBackwards {
        event_id: Uuid,
        previous: i64,
        found: i64,
    },
}
