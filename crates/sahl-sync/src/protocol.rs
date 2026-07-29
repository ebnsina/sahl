//! Wire types. Changing any of these is a protocol version bump — tills in the field outlive
//! server deploys.
//!
//! snake_case throughout, matching `EventEnvelope`, whose encoding is fixed by the hash. This
//! protocol is Rust-to-Rust; the camelCase boundary is the terminal's command layer, not here.

use sahl_core::event::{ChainTip, EventEnvelope};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::SyncError;

/// Most events in one push.
///
/// Bounded so a terminal that has been offline for a week uploads in chunks rather than one request
/// that times out and retries forever, making no progress. 500 is roughly a busy day's events.
pub const MAX_BATCH: usize = 500;

/// A terminal offering events to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushRequest {
    pub device_id: Uuid,
    /// Contiguous and ascending. The server rejects gaps rather than storing them.
    pub events: Vec<EventEnvelope>,
}

impl PushRequest {
    /// # Errors
    /// [`SyncError::BatchTooLarge`] if the batch exceeds [`MAX_BATCH`].
    pub fn validate(&self) -> Result<(), SyncError> {
        if self.events.len() > MAX_BATCH {
            return Err(SyncError::BatchTooLarge {
                count: self.events.len(),
                limit: MAX_BATCH,
            });
        }
        Ok(())
    }
}

/// What the server did with a push.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushResponse {
    /// Events newly stored. Zero on a retry, which is success, not failure.
    pub accepted: usize,
    /// Events already held, skipped.
    pub skipped: usize,
    /// The server's view of the device's chain after this push.
    ///
    /// The terminal compares this against its own tip. Agreement is what lets it mark events synced;
    /// disagreement means a truncated local log, which no hash check alone can detect.
    pub tip: ChainTip,
    /// Highest `server_seq` assigned, so the terminal can advance its pull cursor without a
    /// round trip for events it just sent.
    pub high_water: i64,
}

/// A terminal asking for everything it has not seen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub device_id: Uuid,
    /// Everything strictly above this. Zero fetches from the beginning.
    pub cursor: i64,
    pub limit: usize,
}

/// Events from the outlet's other devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullResponse {
    pub events: Vec<EventEnvelope>,
    /// Where to resume. Sent explicitly rather than inferred from the last event, so an empty page
    /// still advances past events filtered out server-side.
    pub cursor: i64,
    /// Whether more is waiting, so the terminal can drain without guessing.
    pub has_more: bool,
}

/// A device's position in the outlet's stream.
///
/// Per device rather than per outlet: two tills in one shop sync independently, and one being
/// offline must not hold back the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCursor {
    pub device_id: Uuid,
    pub server_seq: i64,
}

/// Why a push was refused, in a form the terminal can act on.
///
/// Deliberately coarse. A terminal cannot repair a fork or a gap on its own, so the distinction
/// that matters is "retry later" versus "stop and call support" — not the precise variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRejection {
    /// Device unknown or revoked. Stop trying; the terminal needs re-enrolling.
    NotAuthorised,
    /// Signature, timestamp, or chain verification failed. Not retryable.
    Invalid,
    /// Local log disagrees with the server's history. Needs an operator.
    Forked,
    /// Events are missing. The terminal must send earlier ones first.
    Gap,
    /// Server-side problem. Retry with backoff.
    Unavailable,
}

impl SyncRejection {
    /// Whether sending the same batch again could ever succeed.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::Unavailable)
    }

    #[must_use]
    pub const fn from_sync_error(error: &SyncError) -> Self {
        match error {
            SyncError::Gap { .. } => Self::Gap,
            SyncError::Forked { .. } => Self::Forked,
            SyncError::Chain(_)
            | SyncError::WrongDevice { .. }
            | SyncError::BatchTooLarge { .. } => Self::Invalid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sahl_core::event::EventHash;

    #[test]
    fn only_a_server_problem_is_worth_retrying() {
        // Retrying a fork or a gap forever would just fill logs and never recover.
        assert!(SyncRejection::Unavailable.is_retryable());
        assert!(!SyncRejection::Forked.is_retryable());
        assert!(!SyncRejection::Gap.is_retryable());
        assert!(!SyncRejection::Invalid.is_retryable());
        assert!(!SyncRejection::NotAuthorised.is_retryable());
    }

    #[test]
    fn sync_errors_map_to_actionable_rejections() {
        assert_eq!(
            SyncRejection::from_sync_error(&SyncError::Gap {
                tip: 3,
                batch_starts_at: 7
            }),
            SyncRejection::Gap
        );
        assert_eq!(
            SyncRejection::from_sync_error(&SyncError::Forked {
                device_seq: 3,
                server_hash: EventHash::GENESIS,
                device_hash: EventHash::GENESIS,
            }),
            SyncRejection::Forked
        );
    }

    #[test]
    fn an_oversized_batch_is_refused_before_any_work() {
        // Bounded so a till offline for a week uploads in chunks rather than one request that
        // times out forever, making no progress.
        let mut chain = sahl_core::event::EventChain::new(Uuid::from_u128(1));
        let one = chain
            .append(
                sahl_core::event::EventHeader {
                    event_id: Uuid::from_u128(9),
                    tenant_id: Uuid::from_u128(2),
                    outlet_id: Uuid::from_u128(3),
                    device_id: Uuid::from_u128(1),
                    occurred_at: sahl_core::Timestamp::from_millis(0),
                },
                &Probe,
            )
            .expect("seals");

        let at_limit = PushRequest {
            device_id: Uuid::from_u128(1),
            events: vec![one.clone(); MAX_BATCH],
        };
        assert_eq!(at_limit.validate(), Ok(()));

        let over = PushRequest {
            device_id: Uuid::from_u128(1),
            events: vec![one; MAX_BATCH + 1],
        };
        assert_eq!(
            over.validate(),
            Err(SyncError::BatchTooLarge {
                count: MAX_BATCH + 1,
                limit: MAX_BATCH
            })
        );
    }

    #[derive(serde::Serialize)]
    struct Probe;
    impl sahl_core::event::EventPayload for Probe {
        fn kind(&self) -> &'static str {
            "test.probe"
        }
    }

    #[test]
    fn the_wire_format_is_consistently_snake_case() {
        // A payload mixing cases is a trap for every future client. EventEnvelope fixes the style.
        let response = PushResponse {
            accepted: 3,
            skipped: 1,
            tip: ChainTip::GENESIS,
            high_water: 42,
        };
        let encoded = serde_json::to_string(&response).expect("serialises");

        assert!(encoded.contains("\"high_water\":42"));
        assert!(
            encoded.contains("\"device_seq\":0"),
            "nested ChainTip must match the wrapper"
        );
    }

    #[test]
    fn a_pull_response_round_trips() {
        let response = PullResponse {
            events: Vec::new(),
            cursor: 17,
            has_more: false,
        };
        let encoded = serde_json::to_string(&response).expect("serialises");
        let decoded: PullResponse = serde_json::from_str(&encoded).expect("deserialises");

        assert_eq!(decoded.cursor, 17);
        assert!(!decoded.has_more);
    }
}
