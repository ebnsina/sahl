use sahl_core::event::{EventError, EventHash};
use thiserror::Error;
use uuid::Uuid;

/// Why a sync batch was refused.
///
/// Every variant means the server declined to record something. None are retryable by simply
/// sending the same bytes again — they need operator attention.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SyncError {
    #[error("chain error: {0}")]
    Chain(#[from] EventError),

    /// Events are missing between the server's tip and the batch.
    #[error("batch starts at {batch_starts_at} but the chain is at {tip}; events are missing")]
    Gap { tip: u64, batch_starts_at: u64 },

    /// Same sequence numbers, different history — a restored backup, or tampering.
    #[error(
        "device forked at sequence {device_seq}: server has {server_hash}, device sent {device_hash}"
    )]
    Forked {
        device_seq: u64,
        server_hash: EventHash,
        device_hash: EventHash,
    },

    #[error("batch contains an event from device {found}, expected {expected}")]
    WrongDevice { expected: Uuid, found: Uuid },

    #[error("batch of {count} events exceeds the limit of {limit}")]
    BatchTooLarge { count: usize, limit: usize },
}
