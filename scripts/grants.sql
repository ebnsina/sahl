-- What the runtime role may touch.
--
-- One file, because this list lived in four places — the dev script, two CI steps and the
-- migrations README — and a table added to the schema reached three of them. CI went red on a
-- permission error that no local run could reproduce, which is the specific waste this prevents.
--
-- The role is deliberately unprivileged: NOSUPERUSER NOBYPASSRLS, no DDL. That is what makes the
-- row-level security policies mean anything, and it is why every new table needs a line here.
GRANT USAGE ON SCHEMA public TO sahl_app;

GRANT SELECT, INSERT, UPDATE
    ON tenant, outlet, app_user, device, enrollment_token, dashboard_token
    TO sahl_app;

-- Append-only by policy as well as by intent: no UPDATE, no DELETE.
GRANT SELECT, INSERT ON event TO sahl_app;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO sahl_app;

-- Each of these is REVOKEd from PUBLIC by its own migration, so the runtime role needs naming
-- explicitly. They exist because the tenant is exactly what the lookup discovers, so the query
-- cannot be scoped to a tenant first. Without the grants, authentication fails for every request.
GRANT EXECUTE ON FUNCTION device_tenant(UUID) TO sahl_app;
GRANT EXECUTE ON FUNCTION enrollment_token_for_digest(BYTEA) TO sahl_app;
GRANT EXECUTE ON FUNCTION dashboard_token_for_digest(BYTEA) TO sahl_app;
GRANT EXECUTE ON FUNCTION outlet_tenant(UUID) TO sahl_app;

-- Startup verifies the schema is current before serving, as the runtime role. Only present where
-- the schema was applied by `sahl-server migrate` rather than by piping the SQL files in, so this
-- is conditional — a missing grant here refuses to boot with what reads like a database outage.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = '_sqlx_migrations') THEN
        GRANT SELECT ON _sqlx_migrations TO sahl_app;
    END IF;
END
$$;
