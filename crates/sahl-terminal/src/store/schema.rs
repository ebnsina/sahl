//! Local SQLite schema.
//!
//! Narrower than the server's: one outlet, no tenancy to isolate. The device is the boundary, so
//! the file is encrypted at rest rather than row-secured.

/// Applied on every open. `IF NOT EXISTS` throughout, so reopening is a no-op.
pub const SCHEMA: &str = r"
-- Every event the till knows about, its own and its siblings'.
--
-- Keyed by event_id rather than device_seq: sequence numbers are only unique *within* a device, and
-- once a second till in the same shop syncs, its sequence 1 collides with this one's.
CREATE TABLE IF NOT EXISTS event (
    event_id    TEXT    PRIMARY KEY,
    device_id   TEXT    NOT NULL,
    device_seq  INTEGER NOT NULL,
    tenant_id   TEXT    NOT NULL,
    outlet_id   TEXT    NOT NULL,

    -- Milliseconds since the Unix epoch. An integer because this value is hashed.
    occurred_at INTEGER NOT NULL,

    kind        TEXT    NOT NULL,
    -- Canonical JSON exactly as hashed, stored verbatim so the digest recomputes byte for byte.
    payload     TEXT    NOT NULL,

    prev_hash   BLOB    NOT NULL,
    hash        BLOB    NOT NULL,

    -- 'local' events this device sealed; 'remote' ones pulled from a sibling till.
    origin      TEXT    NOT NULL CHECK (origin IN ('local', 'remote')),

    -- Local only: NULL until the server accepts it. This column is the whole sync queue.
    synced_at   INTEGER,
    -- Remote only: position in the outlet stream, so the pull cursor can advance.
    server_seq  INTEGER,

    UNIQUE (device_id, device_seq)
);

-- The push queue: local events still outstanding, oldest first.
CREATE INDEX IF NOT EXISTS event_unsynced_idx
    ON event (device_seq) WHERE origin = 'local' AND synced_at IS NULL;

CREATE INDEX IF NOT EXISTS event_chain_idx ON event (device_id, device_seq);
CREATE INDEX IF NOT EXISTS event_kind_idx ON event (kind);

-- Single-row table holding the pull cursor. The CHECK keeps it single-row.
CREATE TABLE IF NOT EXISTS sync_state (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    pull_cursor INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO sync_state (id, pull_cursor) VALUES (1, 0);
";

/// Enforced on every connection.
///
/// `synchronous = FULL` costs throughput to guarantee a committed sale survives losing power
/// mid-transaction — not hypothetical in the target market.
pub const PRAGMAS: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
";
