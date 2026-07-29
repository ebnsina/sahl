//! Local SQLite schema.
//!
//! Deliberately narrower than the server's: a terminal holds one device's chain for one outlet, so
//! there is no tenancy to isolate here. The physical device *is* the boundary, which is why the
//! file is encrypted at rest rather than row-secured.

/// Applied on every open. `IF NOT EXISTS` throughout, so opening an existing store is a no-op.
pub const SCHEMA: &str = r"
-- The device's own append-only chain. `device_seq` is the primary key, so it is both the ordering
-- and a guarantee that a sequence number cannot repeat.
CREATE TABLE IF NOT EXISTS event (
    device_seq  INTEGER PRIMARY KEY,
    event_id    TEXT    NOT NULL UNIQUE,
    tenant_id   TEXT    NOT NULL,
    outlet_id   TEXT    NOT NULL,
    device_id   TEXT    NOT NULL,

    -- Milliseconds since the Unix epoch, matching sahl_core::Timestamp. An integer because this
    -- value is hashed, and a formatted datetime has no single representation.
    occurred_at INTEGER NOT NULL,

    kind        TEXT    NOT NULL,
    -- Canonical JSON exactly as hashed. Stored verbatim so the digest can be recomputed byte for
    -- byte; re-serialising on read would risk a different encoding and a chain that fails to verify.
    payload     TEXT    NOT NULL,

    prev_hash   BLOB    NOT NULL,
    hash        BLOB    NOT NULL,

    -- NULL until the server has accepted it. This column is the entire sync queue: 'what have I not
    -- pushed yet' is a WHERE clause, not a separate table that could drift out of step with the log.
    synced_at   INTEGER
);

-- The push query: everything still outstanding, oldest first.
CREATE INDEX IF NOT EXISTS event_unsynced_idx ON event (device_seq) WHERE synced_at IS NULL;

CREATE INDEX IF NOT EXISTS event_kind_idx ON event (kind);
";

/// Enforced on every connection.
///
/// `foreign_keys` is off by default in SQLite, and `synchronous = FULL` costs a little throughput to
/// guarantee a committed sale survives losing power mid-transaction — which in the target market is
/// not a hypothetical. A till that loses the last sale in a blackout is exactly the failure this
/// product exists to prevent.
pub const PRAGMAS: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
";
