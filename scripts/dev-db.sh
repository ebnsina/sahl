#!/usr/bin/env bash
# Create the local Postgres the database-backed tests need.
#
# Without SAHL_TEST_DATABASE_URL those tests return early and still report "ok" — green with no
# coverage, which is worse than a failure because nothing draws attention to it. CI always sets the
# variable; this is how a developer gets the same thing locally.
#
#   ./scripts/dev-db.sh
#   export SAHL_TEST_DATABASE_URL="postgres://sahl_app:dev@localhost:5432/sahl_dev"
set -euo pipefail

DB="${1:-sahl_dev}"

# The runtime role is NOSUPERUSER NOBYPASSRLS on purpose. Running these tests as a superuser
# silently bypasses every row-level security policy, so they would pass no matter what the policies
# said — which is exactly the failure the isolation tests exist to catch.
psql -d postgres -c "DROP DATABASE IF EXISTS ${DB}" >/dev/null
psql -d postgres -c "CREATE DATABASE ${DB}" >/dev/null

for file in crates/sahl-server/migrations/*.sql; do
  psql -v ON_ERROR_STOP=1 -d "${DB}" -f "${file}" >/dev/null
done

# Created only if absent. A role is cluster-wide, so dropping it would break every other database
# that has already granted to it — which is what happens the second time this script is run for a
# different database name.
psql -v ON_ERROR_STOP=1 -d "${DB}" <<SQL >/dev/null
DO \$\$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'sahl_app') THEN
    CREATE ROLE sahl_app LOGIN PASSWORD 'dev' NOSUPERUSER NOBYPASSRLS;
  END IF;
END
\$\$;
GRANT USAGE ON SCHEMA public TO sahl_app;
GRANT SELECT, INSERT, UPDATE ON tenant, outlet, app_user, device, enrollment_token, dashboard_token TO sahl_app;
GRANT SELECT, INSERT ON event TO sahl_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO sahl_app;
-- REVOKEd from PUBLIC by migration 0002, so the runtime role needs it explicitly. Without this
-- every request fails authentication, and only a non-superuser run reveals that.
GRANT EXECUTE ON FUNCTION device_tenant(UUID) TO sahl_app;
GRANT EXECUTE ON FUNCTION enrollment_token_for_digest(BYTEA) TO sahl_app;
-- Same shape and same reason as the enrollment lookup: the tenant is only known *from* the row,
-- so the digest lookup has to run before the transaction can be scoped to anything.
GRANT EXECUTE ON FUNCTION dashboard_token_for_digest(BYTEA) TO sahl_app;
GRANT EXECUTE ON FUNCTION outlet_tenant(UUID) TO sahl_app;
SQL

echo "ready:  export SAHL_TEST_DATABASE_URL=\"postgres://sahl_app:dev@localhost:5432/${DB}\""
