-- Resolve an enrollment token by digest, before any tenant is known.
--
-- The second query that cannot be tenant-scoped: the tenant is exactly what redeeming the token
-- discovers. Narrow by construction — it takes a 32-byte digest, which a caller can only produce by
-- already holding the token, and returns nothing else about the tenant.
CREATE FUNCTION enrollment_token_for_digest(lookup_digest BYTEA)
RETURNS TABLE (
    id                UUID,
    tenant_id         UUID,
    outlet_id         UUID,
    expires_at_millis BIGINT,
    consumed          BOOLEAN
)
    LANGUAGE sql
    STABLE
    SECURITY DEFINER
    SET search_path = public, pg_temp
AS $$
    SELECT
        t.id,
        t.tenant_id,
        t.outlet_id,
        (extract(epoch FROM t.expires_at) * 1000)::BIGINT,
        t.consumed_at IS NOT NULL
    FROM enrollment_token t
    WHERE t.token_hash = lookup_digest;
$$;

REVOKE ALL ON FUNCTION enrollment_token_for_digest(BYTEA) FROM PUBLIC;
