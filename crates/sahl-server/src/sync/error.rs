use sahl_sync::{SyncError, SyncRejection};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("unknown device {device_id}")]
    UnknownDevice { device_id: Uuid },

    #[error("device {device_id} is revoked")]
    Revoked { device_id: Uuid },

    #[error("device {device_id} row is corrupt")]
    CorruptDevice { device_id: Uuid },

    #[error("a stored event has a malformed hash")]
    CorruptEvent,

    /// The device sent events for a tenant or outlet it does not belong to.
    #[error("device {device_id} sent event {event_id} outside its own outlet")]
    ScopeMismatch { device_id: Uuid, event_id: Uuid },

    #[error("{0}")]
    Rejected(SyncError),
}

impl IngestError {
    /// The coarse reason handed back to the terminal.
    ///
    /// Deliberately lossy: telling a client exactly why it failed helps an attacker more than it
    /// helps a cashier.
    #[must_use]
    pub const fn rejection(&self) -> SyncRejection {
        match self {
            Self::UnknownDevice { .. } | Self::Revoked { .. } => SyncRejection::NotAuthorised,
            Self::ScopeMismatch { .. } => SyncRejection::Invalid,
            Self::Database(_) | Self::CorruptDevice { .. } | Self::CorruptEvent => {
                SyncRejection::Unavailable
            }
            Self::Rejected(error) => SyncRejection::from_sync_error(error),
        }
    }
}
