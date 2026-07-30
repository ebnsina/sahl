# Migrations and database roles

Migrations are an explicit deploy step, **not** something normal startup does:

```sh
sahl-server migrate     # as a DDL-capable role
sahl-server             # normal startup: verifies, then serves
```

Two reasons, pulling the same way. The runtime role deliberately holds no DDL rights — that is what
makes the RLS policies meaningful — and several replicas starting at once would race to migrate the
same database. Normal startup instead *verifies* every embedded migration is applied and refuses to
serve if the schema is behind, so a missing column becomes a boot failure rather than a request
failure hours later in front of a customer.

## The role the server connects as MUST NOT bypass RLS

Every tenant-scoped table has row-level security with `FORCE ROW LEVEL SECURITY`, so even the table
owner is subject to its policies. **Superusers bypass RLS regardless**, and so does any role with
`BYPASSRLS`. Connecting as one of those leaves the code looking correct while every policy in the
schema does nothing — the kind of misconfiguration that surfaces as a cross-merchant data leak.

`sahl-server` checks this at startup and refuses to serve if the role is unsafe. Create it once, as
a superuser, before first deploy:

```sql
CREATE ROLE sahl_app LOGIN PASSWORD '...' NOSUPERUSER NOBYPASSRLS;

-- The rest of the list lives in scripts/grants.sql, applied by the dev script and by CI so
-- the three cannot drift. A table added to the schema needs a line there, or the runtime
-- role fails on it with a permission error nobody can reproduce locally.
\i scripts/grants.sql
```

Migrations themselves need DDL rights, so run them as the owning role and let the server connect as
`sahl_app`.

## Tenant scoping

Never query tenant data outside `db::begin_for_tenant`. It sets `sahl.tenant_id` as a
**transaction-local** setting, so a pooled connection cannot carry one merchant's scope into the
next request. With the setting unset, policies evaluate `tenant_id = NULL` and return nothing —
forgetting to scope produces a visibly empty result, never another merchant's rows.

## Issuing enrollment tokens

Until owners can do this from the dashboard — which needs the staff authentication that lands in
P3 — tokens are issued from the CLI:

```sh
sahl-server issue-token <outlet-id> [ttl-seconds]   # defaults to 900
```

The plaintext is printed to stdout **once** and never logged. The database stores only its SHA-256
digest, so a lost token is reissued rather than recovered.
