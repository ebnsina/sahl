-- Resolve a device's tenant so a request can be scoped before RLS applies.
--
-- Chicken and egg: every policy keys off sahl.tenant_id, but the tenant is only knowable from the
-- device row, which is itself behind RLS. This is the one lookup that must run unscoped.
--
-- SECURITY DEFINER with a fixed search_path, returning exactly one column for one primary key.
-- It widens nothing else, and a caller learns only a tenant id it must already possess a signing
-- key for to do anything with.
CREATE FUNCTION device_tenant(lookup_device_id UUID) RETURNS UUID
    LANGUAGE sql
    STABLE
    SECURITY DEFINER
    SET search_path = public, pg_temp
AS $$
    SELECT tenant_id FROM device WHERE id = lookup_device_id;
$$;

REVOKE ALL ON FUNCTION device_tenant(UUID) FROM PUBLIC;
