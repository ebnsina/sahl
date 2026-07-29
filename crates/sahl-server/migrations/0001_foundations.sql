-- Sahl foundations: tenancy, outlets, staff, devices, and the event log.
--
-- Two things here are structural rather than incidental.
--
-- **Row-level security is on every tenant-scoped table**, and the application connects as a role
-- without BYPASSRLS. Application-level scoping is still applied in every query, but RLS is what makes
-- a forgotten WHERE clause a failed query instead of a cross-merchant data leak. `current_setting`
-- with `missing_ok = true` returns NULL when the tenant is unset, and `tenant_id = NULL` is never
-- true — so the policies fail closed.
--
-- **The event log is append-only.** No UPDATE or DELETE grant is issued for it, and a trigger blocks
-- both regardless. The hash chain would detect tampering after the fact; this prevents it.

-- ---------------------------------------------------------------------------------------------
-- Tenancy
-- ---------------------------------------------------------------------------------------------

CREATE TABLE tenant (
    id               UUID PRIMARY KEY,
    name             TEXT        NOT NULL CHECK (length(trim(name)) > 0),
    -- ISO-3166 alpha-2. Drives which fiscalization adapter applies.
    country_code     CHAR(2)     NOT NULL CHECK (country_code ~ '^[A-Z]{2}$'),
    -- ISO-4217, must match sahl_core::money::Currency.
    default_currency CHAR(3)     NOT NULL CHECK (default_currency ~ '^[A-Z]{3}$'),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    suspended_at     TIMESTAMPTZ
);

CREATE TABLE outlet (
    id           UUID PRIMARY KEY,
    tenant_id    UUID        NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    name         TEXT        NOT NULL CHECK (length(trim(name)) > 0),
    -- The vertical profile. One codebase; this row decides which capabilities are on.
    profile      TEXT        NOT NULL CHECK (profile IN ('retail', 'cafe', 'grocery')),
    -- IANA zone. Required, never defaulted: a POS reports by business day, and guessing the zone
    -- silently mis-assigns late-evening sales to the wrong day.
    timezone     TEXT        NOT NULL CHECK (length(trim(timezone)) > 0),
    currency     CHAR(3)     NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    -- Both target markets price at retail inclusive of VAT.
    pricing_mode TEXT        NOT NULL DEFAULT 'tax_inclusive'
                 CHECK (pricing_mode IN ('tax_inclusive', 'tax_exclusive')),
    -- Whether a sale may proceed past available stock. Merchants disagree; both are defensible.
    allow_oversell BOOLEAN   NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at    TIMESTAMPTZ
);

CREATE INDEX outlet_tenant_idx ON outlet (tenant_id);

CREATE TABLE app_user (
    id         UUID PRIMARY KEY,
    tenant_id  UUID        NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    -- NULL means tenant-wide access (an owner across every outlet).
    outlet_id  UUID        REFERENCES outlet (id) ON DELETE CASCADE,
    name       TEXT        NOT NULL CHECK (length(trim(name)) > 0),
    role       TEXT        NOT NULL CHECK (role IN ('owner', 'manager', 'cashier')),
    -- Argon2id. A till PIN is short and brute-forceable by construction, so the KDF cost is what
    -- protects it; never store or log the PIN itself.
    pin_hash   TEXT        NOT NULL,
    active     BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX app_user_tenant_idx ON app_user (tenant_id);

-- ---------------------------------------------------------------------------------------------
-- Devices
-- ---------------------------------------------------------------------------------------------

-- A single-use, short-lived credential an owner generates to enrol a terminal.
--
-- Only the token's SHA-256 digest is stored. A leaked database backup therefore does not let anyone
-- enrol a device, and the plaintext exists exactly once, in the response that created it.
CREATE TABLE enrollment_token (
    id                UUID PRIMARY KEY,
    tenant_id         UUID        NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    outlet_id         UUID        NOT NULL REFERENCES outlet (id) ON DELETE CASCADE,
    token_hash        BYTEA       NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    expires_at        TIMESTAMPTZ NOT NULL,
    consumed_at       TIMESTAMPTZ,
    consumed_by       UUID,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX enrollment_token_tenant_idx ON enrollment_token (tenant_id);

-- An enrolled terminal.
--
-- `public_key` is the device's Ed25519 verifying key. The private half is generated on the terminal
-- and stored in the OS keychain — it never transits the network and the server never sees it, so a
-- server compromise cannot forge a device's events.
CREATE TABLE device (
    id              UUID PRIMARY KEY,
    tenant_id       UUID        NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    outlet_id       UUID        NOT NULL REFERENCES outlet (id) ON DELETE CASCADE,
    label           TEXT        NOT NULL CHECK (length(trim(label)) > 0),
    public_key      BYTEA       NOT NULL CHECK (length(public_key) = 32),
    enrolled_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at    TIMESTAMPTZ,
    -- Revocation is immediate and server-side: a stolen terminal stops syncing on the next request.
    revoked_at      TIMESTAMPTZ,

    -- The device's chain tip as the server last accepted it. Comparing a sync batch against this is
    -- what catches a truncated log — deleting events from the end leaves a valid prefix that no hash
    -- check alone can detect.
    last_device_seq BIGINT      NOT NULL DEFAULT 0 CHECK (last_device_seq >= 0),
    last_hash       BYTEA       NOT NULL DEFAULT decode(repeat('00', 32), 'hex')
                    CHECK (length(last_hash) = 32)
);

CREATE INDEX device_tenant_idx ON device (tenant_id);
CREATE INDEX device_outlet_idx ON device (outlet_id);

ALTER TABLE enrollment_token
    ADD CONSTRAINT enrollment_token_consumed_by_fk
    FOREIGN KEY (consumed_by) REFERENCES device (id) ON DELETE SET NULL;

-- ---------------------------------------------------------------------------------------------
-- The event log
-- ---------------------------------------------------------------------------------------------

CREATE TABLE event (
    -- UUID v7 from the terminal: globally unique, time-sortable, and the idempotency key that makes
    -- a retried sync batch harmless.
    event_id    UUID    PRIMARY KEY,
    tenant_id   UUID    NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    outlet_id   UUID    NOT NULL REFERENCES outlet (id) ON DELETE CASCADE,
    device_id   UUID    NOT NULL REFERENCES device (id) ON DELETE RESTRICT,

    -- Monotonic per device from 1. The UNIQUE constraint below makes a gap or a repeat impossible to
    -- store, not merely detectable.
    device_seq  BIGINT  NOT NULL CHECK (device_seq > 0),

    -- Milliseconds since the Unix epoch, matching sahl_core::time::Timestamp. Stored as an integer
    -- rather than a timestamptz because this value is hashed: a formatted datetime that round-trips
    -- differently between two libraries would break the chain.
    occurred_at BIGINT  NOT NULL,
    -- The server's own clock, kept separate. Device clocks drift and are a fraud signal in their own
    -- right, so the terminal's claim is never overwritten.
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    kind        TEXT    NOT NULL CHECK (length(trim(kind)) > 0),
    payload     JSONB   NOT NULL,

    prev_hash   BYTEA   NOT NULL CHECK (length(prev_hash) = 32),
    hash        BYTEA   NOT NULL CHECK (length(hash) = 32),

    -- Assigned by the server on ingest. Terminals pull everything above their cursor, which is how a
    -- second till in the same shop learns about the first till's sales.
    server_seq  BIGSERIAL NOT NULL,

    CONSTRAINT event_device_sequence_unique UNIQUE (device_id, device_seq)
);

CREATE INDEX event_pull_idx ON event (tenant_id, server_seq);
CREATE INDEX event_outlet_pull_idx ON event (outlet_id, server_seq);
CREATE INDEX event_device_idx ON event (device_id, device_seq);

-- Append-only, enforced rather than trusted.
--
-- The hash chain makes tampering *detectable*; this makes it impossible through the normal path. A
-- correction is expressed as a new compensating event, exactly as a ledger requires — which is also
-- what keeps "replay the day and prove the numbers" true.
CREATE FUNCTION event_is_append_only() RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION
        'the event log is append-only: % is not permitted. Record a compensating event instead.',
        TG_OP;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER event_no_update BEFORE UPDATE ON event
    FOR EACH ROW EXECUTE FUNCTION event_is_append_only();

CREATE TRIGGER event_no_delete BEFORE DELETE ON event
    FOR EACH ROW EXECUTE FUNCTION event_is_append_only();

-- ---------------------------------------------------------------------------------------------
-- Row-level security
--
-- Every policy reads `sahl.tenant_id`, which the application sets per transaction via
-- `set_config('sahl.tenant_id', $1, true)`. The `true` makes it transaction-local, so a pooled
-- connection cannot leak one merchant's scope into the next request.
--
-- `current_setting(..., true)` yields NULL when unset, and `tenant_id = NULL` is never true, so an
-- unscoped connection sees nothing. Fail closed, not open.
-- ---------------------------------------------------------------------------------------------

CREATE FUNCTION current_tenant_id() RETURNS UUID AS $$
    SELECT nullif(current_setting('sahl.tenant_id', true), '')::UUID;
$$ LANGUAGE sql STABLE;

ALTER TABLE tenant           ENABLE ROW LEVEL SECURITY;
ALTER TABLE outlet           ENABLE ROW LEVEL SECURITY;
ALTER TABLE app_user         ENABLE ROW LEVEL SECURITY;
ALTER TABLE device           ENABLE ROW LEVEL SECURITY;
ALTER TABLE enrollment_token ENABLE ROW LEVEL SECURITY;
ALTER TABLE event            ENABLE ROW LEVEL SECURITY;

-- FORCE applies the policies to the table owner too. Without it, the role that owns these tables
-- bypasses RLS entirely and the protection is theatre in exactly the deployment most likely to use
-- a single database role.
ALTER TABLE tenant           FORCE ROW LEVEL SECURITY;
ALTER TABLE outlet           FORCE ROW LEVEL SECURITY;
ALTER TABLE app_user         FORCE ROW LEVEL SECURITY;
ALTER TABLE device           FORCE ROW LEVEL SECURITY;
ALTER TABLE enrollment_token FORCE ROW LEVEL SECURITY;
ALTER TABLE event            FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON tenant
    USING (id = current_tenant_id())
    WITH CHECK (id = current_tenant_id());

CREATE POLICY tenant_isolation ON outlet
    USING (tenant_id = current_tenant_id())
    WITH CHECK (tenant_id = current_tenant_id());

CREATE POLICY tenant_isolation ON app_user
    USING (tenant_id = current_tenant_id())
    WITH CHECK (tenant_id = current_tenant_id());

CREATE POLICY tenant_isolation ON device
    USING (tenant_id = current_tenant_id())
    WITH CHECK (tenant_id = current_tenant_id());

CREATE POLICY tenant_isolation ON enrollment_token
    USING (tenant_id = current_tenant_id())
    WITH CHECK (tenant_id = current_tenant_id());

CREATE POLICY tenant_isolation ON event
    USING (tenant_id = current_tenant_id())
    WITH CHECK (tenant_id = current_tenant_id());
