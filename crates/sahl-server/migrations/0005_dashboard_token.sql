-- How an owner reads their own numbers from a phone.
--
-- A till proves who it is with an Ed25519 keypair in the OS keychain. An owner on a phone has no
-- keychain and no device row, so they need a different credential — and it must not be their till
-- PIN. A PIN is four digits by design: brute-forceable by construction, protected only by the KDF
-- cost and by the fact that guessing it requires standing at the counter. Putting one on a public
-- HTTP endpoint removes the only part of that which was doing real work.
--
-- So: a long random token, exactly as enrollment already does. Only the SHA-256 digest is stored,
-- so a leaked backup does not grant access, and the plaintext exists once — in the output of the
-- command that created it.
--
-- Unlike an enrollment token this one is not single-use and does not expire on a timer: it is how
-- somebody reads their shop every morning. It is revoked instead, which is a decision somebody
-- makes rather than a clock running out at an unhelpful moment.
CREATE TABLE dashboard_token (
    id          UUID PRIMARY KEY,
    tenant_id   UUID        NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    -- NULL means every outlet in the tenant, matching app_user.outlet_id.
    outlet_id   UUID        REFERENCES outlet (id) ON DELETE CASCADE,
    -- What to call it in a list of tokens, so revoking the right one is possible.
    label       TEXT        NOT NULL CHECK (length(trim(label)) > 0),
    token_hash  BYTEA       NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    revoked_at  TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX dashboard_token_tenant_idx ON dashboard_token (tenant_id);

ALTER TABLE dashboard_token ENABLE ROW LEVEL SECURITY;
ALTER TABLE dashboard_token FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON dashboard_token
    USING (tenant_id = current_setting('sahl.tenant_id', TRUE)::UUID)
    WITH CHECK (tenant_id = current_setting('sahl.tenant_id', TRUE)::UUID);

-- Resolve a dashboard token by digest, before any tenant is known.
--
-- Same shape and same reasoning as the enrollment lookup: the tenant is exactly what presenting
-- the token discovers, so this one query cannot be tenant-scoped. Narrow by construction — it
-- takes a 32-byte digest, which a caller can only produce by already holding the token, and
-- returns nothing about the tenant beyond its id.
CREATE FUNCTION dashboard_token_for_digest(lookup_digest BYTEA)
RETURNS TABLE (
    id        UUID,
    tenant_id UUID,
    outlet_id UUID,
    revoked   BOOLEAN
)
    LANGUAGE sql
    STABLE
    SECURITY DEFINER
    SET search_path = public, pg_temp
AS $$
    SELECT t.id, t.tenant_id, t.outlet_id, t.revoked_at IS NOT NULL
    FROM dashboard_token t
    WHERE t.token_hash = lookup_digest;
$$;

REVOKE ALL ON FUNCTION dashboard_token_for_digest(BYTEA) FROM PUBLIC;

-- Resolve an outlet's tenant, before any tenant is known.
--
-- The third and last query that cannot be tenant-scoped, and it exists because the admin commands
-- that mint tokens are handed an outlet id and nothing else. Without it those commands only work
-- as a superuser — which is to say they work in development and fail in production, the worst of
-- the available orders.
--
-- Same shape as `device_tenant`: takes an id the caller already has, returns nothing but the
-- tenant it belongs to.
CREATE FUNCTION outlet_tenant(lookup_outlet UUID)
RETURNS UUID
    LANGUAGE sql
    STABLE
    SECURITY DEFINER
    SET search_path = public, pg_temp
AS $$
    SELECT o.tenant_id FROM outlet o WHERE o.id = lookup_outlet;
$$;

REVOKE ALL ON FUNCTION outlet_tenant(UUID) FROM PUBLIC;
