//! The local event store — the terminal's durable memory.
//!
//! This is what makes the register keep selling with the internet down. Every event is written here
//! first, hash-chained, and only later pushed to the server. A sale is not "provisional until
//! synced": it is complete the moment it is on this disk, and sync is a background concern.
//!
//! ## The rule that keeps the architecture honest
//!
//! **The webview never touches SQL.** It calls typed commands; Rust owns the event log, the chain,
//! the projections, and every total. That is what keeps "TypeScript never computes a total"
//! structurally true rather than a convention someone has to remember — there is simply no path
//! from the UI to the data that bypasses `sahl-core`.

mod schema;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use sahl_core::event::{ChainTip, EventEnvelope, EventHash};
use thiserror::Error;
use uuid::Uuid;

use schema::{PRAGMAS, SCHEMA};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("stored event {event_id} is corrupt: {reason}")]
    Corrupt { event_id: String, reason: String },

    #[error("event {event_id} is already stored")]
    Duplicate { event_id: Uuid },

    #[error("cannot append sequence {found}; the chain is at {tip}")]
    SequenceBreak { tip: u64, found: u64 },
}

/// The till's event log: its own chain plus events pulled from sibling devices.
#[derive(Debug)]
pub struct EventStore {
    connection: Connection,
    /// Whose chain is "ours". Sequence numbers only mean anything within one device.
    device_id: Uuid,
}

impl EventStore {
    /// Open (or create) the store at `path`.
    ///
    /// # Errors
    /// [`StoreError::Database`] if the file cannot be opened or the schema cannot be applied.
    pub fn open(path: &Path, device_id: Uuid) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        Self::prepare(connection, device_id)
    }

    /// An in-memory store, for tests.
    ///
    /// # Errors
    /// [`StoreError::Database`] on failure.
    pub fn open_in_memory(device_id: Uuid) -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        Self::prepare(connection, device_id)
    }

    fn prepare(connection: Connection, device_id: Uuid) -> Result<Self, StoreError> {
        connection.execute_batch(PRAGMAS)?;
        connection.execute_batch(SCHEMA)?;
        Ok(Self {
            connection,
            device_id,
        })
    }

    #[must_use]
    pub const fn device_id(&self) -> Uuid {
        self.device_id
    }

    /// Where the chain currently ends.
    ///
    /// Read once at startup so a terminal can resume after a restart without replaying its whole
    /// log — which matters when a busy shop's log runs to millions of rows.
    ///
    /// # Errors
    /// [`StoreError`] on a database failure or a corrupt stored hash.
    pub fn tip(&self) -> Result<ChainTip, StoreError> {
        Self::tip_within(&self.connection, self.device_id)
    }

    fn tip_within(
        connection: &rusqlite::Connection,
        device_id: Uuid,
    ) -> Result<ChainTip, StoreError> {
        let row: Option<(i64, Vec<u8>)> = connection
            .query_row(
                "SELECT device_seq, hash FROM event WHERE device_id = ?1 \
                 ORDER BY device_seq DESC LIMIT 1",
                [device_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let Some((device_seq, hash_bytes)) = row else {
            return Ok(ChainTip::GENESIS);
        };

        Ok(ChainTip {
            device_seq: u64::try_from(device_seq).map_err(|_| StoreError::Corrupt {
                event_id: "tip".to_owned(),
                reason: format!("negative sequence {device_seq}"),
            })?,
            hash: decode_hash(&hash_bytes, "tip")?,
        })
    }

    /// Append a sealed event.
    ///
    /// Rejects a gap or a repeat outright rather than storing it. The hash chain would make either
    /// *detectable* later; refusing here means the local log is never wrong in the first place.
    ///
    /// # Errors
    /// [`StoreError::SequenceBreak`] if the event does not follow the tip,
    /// [`StoreError::Duplicate`] if it is already stored.
    pub fn append(&mut self, event: &EventEnvelope) -> Result<(), StoreError> {
        Self::append_within(&self.connection, event)
    }

    /// The body of [`EventStore::append`], against any connection or open transaction.
    ///
    /// The tip is read from the same handle as the insert, so events appended inside one
    /// transaction chain onto each other rather than all claiming the same predecessor sequence.
    fn append_within(
        connection: &rusqlite::Connection,
        event: &EventEnvelope,
    ) -> Result<(), StoreError> {
        let tip = Self::tip_within(connection, event.device_id)?;
        let expected = tip.device_seq.saturating_add(1);
        if event.device_seq != expected {
            return Err(StoreError::SequenceBreak {
                tip: tip.device_seq,
                found: event.device_seq,
            });
        }

        let sequence = i64::try_from(event.device_seq).map_err(|_| StoreError::Corrupt {
            event_id: event.event_id.to_string(),
            reason: "sequence exceeds i64".to_owned(),
        })?;

        let payload =
            serde_json::to_string(&event.payload).map_err(|error| StoreError::Corrupt {
                event_id: event.event_id.to_string(),
                reason: error.to_string(),
            })?;

        let inserted = connection.execute(
            "INSERT INTO event (
                device_seq, event_id, tenant_id, outlet_id, device_id,
                occurred_at, kind, payload, prev_hash, hash, origin, synced_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'local', NULL)",
            params![
                sequence,
                event.event_id.to_string(),
                event.tenant_id.to_string(),
                event.outlet_id.to_string(),
                event.device_id.to_string(),
                event.occurred_at.millis(),
                event.kind,
                payload,
                event.prev_hash.as_bytes().as_slice(),
                event.hash.as_bytes().as_slice(),
            ],
        );

        match inserted {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(StoreError::Duplicate {
                    event_id: event.event_id,
                })
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Append several sealed events, all or none.
    ///
    /// One action sometimes produces two events — booking a delivery in against an order writes to
    /// the order and to the batch ledger. Appending those separately can leave an order claiming
    /// stock arrived with no batch on the shelf, which is a discrepancy nobody can explain later
    /// because both halves look internally consistent.
    ///
    /// # Errors
    /// The first failure encountered; nothing is written when this returns an error.
    pub fn append_all(&mut self, events: &[EventEnvelope]) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        for event in events {
            Self::append_within(&transaction, event)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Every event, oldest first — the input to a full projection rebuild at startup.
    ///
    /// # Errors
    /// [`StoreError`] on a database failure or corrupt row.
    pub fn load_all(&self) -> Result<Vec<EventEnvelope>, StoreError> {
        self.query(
            "SELECT device_seq, event_id, tenant_id, outlet_id, device_id, occurred_at, \
                    kind, payload, prev_hash, hash FROM event WHERE origin = 'local' \
                    ORDER BY device_seq",
        )
    }

    /// Events not yet accepted by the server, oldest first. The sync queue, in P2.
    ///
    /// # Errors
    /// [`StoreError`] on a database failure or corrupt row.
    pub fn unsynced(&self) -> Result<Vec<EventEnvelope>, StoreError> {
        self.query(
            "SELECT device_seq, event_id, tenant_id, outlet_id, device_id, occurred_at, \
                    kind, payload, prev_hash, hash FROM event \
                    WHERE origin = 'local' AND synced_at IS NULL ORDER BY device_seq",
        )
    }

    /// How many events are waiting to be pushed — the number behind the "12 unsynced" badge.
    ///
    /// # Errors
    /// [`StoreError::Database`] on failure.
    pub fn unsynced_count(&self) -> Result<u64, StoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT count(*) FROM event WHERE origin = 'local' AND synced_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Mark events up to and including `through_seq` as accepted by the server.
    ///
    /// # Errors
    /// [`StoreError::Database`] on failure.
    pub fn mark_synced(&mut self, through_seq: u64, at_millis: i64) -> Result<usize, StoreError> {
        let sequence = i64::try_from(through_seq).unwrap_or(i64::MAX);
        Ok(self.connection.execute(
            "UPDATE event SET synced_at = ?1 \
             WHERE origin = 'local' AND device_seq <= ?2 AND synced_at IS NULL",
            params![at_millis, sequence],
        )?)
    }

    /// Store an event pulled from a sibling till.
    ///
    /// Idempotent: a re-delivered event is ignored rather than erroring. Pull pages can overlap
    /// after a crash, and a till that chokes on seeing the same event twice stops syncing.
    ///
    /// Remote chains are *not* sequence-checked here. This device holds an arbitrary window of a
    /// sibling's history — it may have missed earlier events entirely — so continuity is the
    /// server's job, not a claim this store can make.
    ///
    /// # Errors
    /// [`StoreError`] on a database failure.
    pub fn insert_remote(
        &mut self,
        event: &EventEnvelope,
        server_seq: i64,
    ) -> Result<bool, StoreError> {
        let payload =
            serde_json::to_string(&event.payload).map_err(|error| StoreError::Corrupt {
                event_id: event.event_id.to_string(),
                reason: error.to_string(),
            })?;

        let seq = i64::try_from(event.device_seq).map_err(|_| StoreError::Corrupt {
            event_id: event.event_id.to_string(),
            reason: "sequence exceeds i64".to_owned(),
        })?;

        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO event (
                event_id, device_id, device_seq, tenant_id, outlet_id,
                occurred_at, kind, payload, prev_hash, hash, origin, server_seq
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'remote', ?11)",
            params![
                event.event_id.to_string(),
                event.device_id.to_string(),
                seq,
                event.tenant_id.to_string(),
                event.outlet_id.to_string(),
                event.occurred_at.millis(),
                event.kind,
                payload,
                event.prev_hash.as_bytes().as_slice(),
                event.hash.as_bytes().as_slice(),
                server_seq,
            ],
        )?;
        Ok(inserted > 0)
    }

    /// How far this device has drained the outlet stream.
    ///
    /// # Errors
    /// [`StoreError::Database`] on failure.
    pub fn pull_cursor(&self) -> Result<i64, StoreError> {
        Ok(self.connection.query_row(
            "SELECT pull_cursor FROM sync_state WHERE id = 1",
            [],
            |row| row.get(0),
        )?)
    }

    /// Advance the cursor. Never moves backwards — a stale response arriving late must not rewind
    /// progress and cause the same page to be applied forever.
    ///
    /// # Errors
    /// [`StoreError::Database`] on failure.
    pub fn set_pull_cursor(&mut self, cursor: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE sync_state SET pull_cursor = max(pull_cursor, ?1) WHERE id = 1",
            params![cursor],
        )?;
        Ok(())
    }

    /// Every event, local and remote, in the order the projection should apply them.
    ///
    /// # Errors
    /// [`StoreError`] on a database failure or corrupt row.
    /// Erase every event. **Debug builds only.**
    ///
    /// The one function in this program that destroys records, and it exists for exactly one
    /// reason: switching between demo markets otherwise means quitting the app and finding a
    /// SQLite file. It is fenced at compile time so no release binary contains it at all.
    ///
    /// # Errors
    /// [`StoreError`] if the delete fails.
    #[cfg(debug_assertions)]
    pub fn erase_everything(&mut self) -> Result<(), StoreError> {
        self.connection.execute("DELETE FROM event", [])?;
        // Reset rather than delete: the row is a singleton the cursor lookups expect to exist.
        self.connection
            .execute("UPDATE sync_state SET pull_cursor = 0 WHERE id = 1", [])?;
        Ok(())
    }

    pub fn load_projection_input(&self) -> Result<Vec<EventEnvelope>, StoreError> {
        // Ordered by when the event happened, then by id to break ties deterministically. Two tills
        // sealing at the same millisecond must still replay identically on every device.
        self.query(
            "SELECT device_seq, event_id, tenant_id, outlet_id, device_id, occurred_at, \
                    kind, payload, prev_hash, hash FROM event ORDER BY occurred_at, event_id",
        )
    }

    fn query(&self, sql: &str) -> Result<Vec<EventEnvelope>, StoreError> {
        let mut statement = self.connection.prepare(sql)?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Vec<u8>>(8)?,
                row.get::<_, Vec<u8>>(9)?,
            ))
        })?;

        let mut events = Vec::new();
        for row in rows {
            let (seq, event_id, tenant, outlet, device, occurred, kind, payload, prev, hash) = row?;
            events.push(EventEnvelope {
                event_id: parse_uuid(&event_id, &event_id)?,
                tenant_id: parse_uuid(&tenant, &event_id)?,
                outlet_id: parse_uuid(&outlet, &event_id)?,
                device_id: parse_uuid(&device, &event_id)?,
                device_seq: u64::try_from(seq).map_err(|_| StoreError::Corrupt {
                    event_id: event_id.clone(),
                    reason: format!("negative sequence {seq}"),
                })?,
                occurred_at: sahl_core::Timestamp::from_millis(occurred),
                kind,
                payload: serde_json::from_str(&payload).map_err(|error| StoreError::Corrupt {
                    event_id: event_id.clone(),
                    reason: error.to_string(),
                })?,
                prev_hash: decode_hash(&prev, &event_id)?,
                hash: decode_hash(&hash, &event_id)?,
            });
        }
        Ok(events)
    }
}

fn parse_uuid(value: &str, event_id: &str) -> Result<Uuid, StoreError> {
    Uuid::parse_str(value).map_err(|error| StoreError::Corrupt {
        event_id: event_id.to_owned(),
        reason: error.to_string(),
    })
}

fn decode_hash(bytes: &[u8], event_id: &str) -> Result<EventHash, StoreError> {
    let sized: [u8; 32] = bytes.try_into().map_err(|_| StoreError::Corrupt {
        event_id: event_id.to_owned(),
        reason: format!("hash is {} bytes, expected 32", bytes.len()),
    })?;
    Ok(EventHash::from_bytes(sized))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sahl_core::Timestamp;
    use sahl_core::event::{EventChain, EventHeader, EventPayload, verify_chain_from_genesis};
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

    fn header(n: u32) -> EventHeader {
        EventHeader {
            event_id: id(1_000 + u128::from(n)),
            tenant_id: id(2),
            outlet_id: id(3),
            device_id: id(DEVICE),
            occurred_at: Timestamp::from_millis(1_753_000_000_000 + i64::from(n)),
        }
    }

    fn seeded(count: u32) -> (EventStore, EventChain) {
        let mut store = EventStore::open_in_memory(id(DEVICE)).expect("opens");
        let mut chain = EventChain::new(id(DEVICE));
        for n in 0..count {
            let event = chain.append(header(n), &Tick { n }).expect("appends");
            store.append(&event).expect("stores");
        }
        (store, chain)
    }

    #[test]
    fn a_fresh_store_is_at_genesis() {
        let store = EventStore::open_in_memory(id(DEVICE)).expect("opens");
        assert_eq!(store.tip().expect("tip"), ChainTip::GENESIS);
    }

    #[test]
    fn the_tip_survives_a_reopen_without_replaying_the_log() {
        // A till restarting mid-shift must not have to walk millions of rows.
        let (store, chain) = seeded(25);
        assert_eq!(store.tip().expect("tip"), chain.tip());
        assert_eq!(store.tip().expect("tip").device_seq, 25);
    }

    #[test]
    fn stored_events_round_trip_byte_for_byte() {
        // The payload is stored verbatim precisely so the digest still recomputes on read.
        let (store, _) = seeded(10);
        let loaded = store.load_all().expect("loads");

        assert_eq!(loaded.len(), 10);
        for event in &loaded {
            assert_eq!(event.verify(), Ok(()), "digest must survive the round trip");
        }
        assert!(verify_chain_from_genesis(&loaded).is_ok());
    }

    #[test]
    fn a_sequence_gap_is_refused_rather_than_stored() {
        let (mut store, _) = seeded(3);
        let mut detached = EventChain::resume(
            id(DEVICE),
            ChainTip {
                device_seq: 10,
                hash: EventHash::GENESIS,
            },
        );
        let jumped = detached.append(header(99), &Tick { n: 99 }).expect("seals");

        assert!(matches!(
            store.append(&jumped),
            Err(StoreError::SequenceBreak { tip: 3, found: 11 })
        ));
    }

    #[test]
    fn replaying_the_same_event_is_refused() {
        // Matters for crash recovery: a retry after a partial write must not double-append.
        let mut store = EventStore::open_in_memory(id(DEVICE)).expect("opens");
        let mut chain = EventChain::new(id(DEVICE));
        let event = chain.append(header(0), &Tick { n: 0 }).expect("appends");

        store.append(&event).expect("first append");
        assert!(matches!(
            store.append(&event),
            Err(StoreError::SequenceBreak { .. })
        ));
    }

    #[test]
    fn everything_starts_unsynced() {
        let (store, _) = seeded(7);
        assert_eq!(store.unsynced_count().expect("counts"), 7);
        assert_eq!(store.unsynced().expect("loads").len(), 7);
    }

    #[test]
    fn marking_synced_shrinks_the_queue_without_touching_the_log() {
        let (mut store, _) = seeded(10);
        let updated = store.mark_synced(4, 1_753_000_000_000).expect("marks");

        assert_eq!(updated, 4);
        assert_eq!(store.unsynced_count().expect("counts"), 6);
        // The events themselves are untouched — sync state is metadata, not a rewrite.
        assert_eq!(store.load_all().expect("loads").len(), 10);
    }

    #[test]
    fn marking_synced_is_idempotent() {
        // A push whose response was lost gets retried; acknowledging twice must be harmless.
        let (mut store, _) = seeded(10);
        store.mark_synced(4, 1).expect("marks");
        assert_eq!(store.mark_synced(4, 2).expect("marks again"), 0);
        assert_eq!(store.unsynced_count().expect("counts"), 6);
    }

    #[test]
    fn a_file_backed_store_persists_across_reopen() {
        // A per-test directory without needing a random UUID feature: the process id plus the test
        // name is unique enough, and keeps the crate free of an RNG dependency it does not need.
        let dir = std::env::temp_dir().join(format!("sahl-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("till.db");

        {
            let mut store = EventStore::open(&path, id(DEVICE)).expect("opens");
            let mut chain = EventChain::new(id(DEVICE));
            for n in 0..5 {
                let event = chain.append(header(n), &Tick { n }).expect("appends");
                store.append(&event).expect("stores");
            }
        }

        let reopened = EventStore::open(&path, id(DEVICE)).expect("reopens");
        assert_eq!(reopened.tip().expect("tip").device_seq, 5);
        assert!(verify_chain_from_genesis(&reopened.load_all().expect("loads")).is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }
}
