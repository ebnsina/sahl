//! Database access, and the one helper that makes row-level security actually work.

use sqlx::postgres::PgPoolOptions;
use sqlx::{Acquire, PgPool, Postgres, Transaction};
use uuid::Uuid;

/// The embedded migration set, compiled into the binary.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Build the connection pool.
///
/// Deliberately does **not** run migrations. See [`run_migrations`] for why they are a separate,
/// privileged step.
///
/// # Errors
/// [`sqlx::Error`] if the pool cannot be created or the server is unreachable.
pub async fn connect(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database_url)
        .await
}

/// Apply any outstanding migrations. Requires a role with DDL rights.
///
/// This is an explicit deploy step (`sahl-server migrate`), not something normal startup does, for
/// two reasons that pull in the same direction:
///
/// - **The runtime role deliberately cannot do it.** `sahl_app` is `NOSUPERUSER NOBYPASSRLS` and
///   holds no DDL grants, which is exactly what makes the row-level security policies meaningful.
///   Granting it enough to migrate would undo that.
/// - **Replicas would race.** Several instances starting at once would each try to migrate the same
///   database, which is a well-known way to half-apply a schema change.
///
/// # Errors
/// [`sqlx::Error`] if a migration fails or the role lacks DDL rights.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}

/// Verify every embedded migration has been applied.
///
/// Run at startup in place of migrating. A server whose code expects a column the database does not
/// have will not fail at boot — it will fail on some particular request, hours later, in front of a
/// customer. Checking up front converts that into a refusal to start.
///
/// # Errors
/// [`sqlx::Error`] on query failure. Returns the missing versions when the schema is behind.
pub async fn pending_migrations(pool: &PgPool) -> Result<Vec<i64>, sqlx::Error> {
    // A brand-new database has no `_sqlx_migrations` table at all. That is the very first thing a
    // new operator hits, so it is reported as "nothing applied yet" — which produces the actionable
    // "run migrate first" message — rather than a raw `relation does not exist` from Postgres.
    let table_exists: (bool,) =
        sqlx::query_as("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(pool)
            .await?;

    let applied: std::collections::HashSet<i64> = if table_exists.0 {
        sqlx::query_as::<_, (i64,)>("SELECT version FROM _sqlx_migrations")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(version,)| version)
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    Ok(MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .filter(|version| !applied.contains(version))
        .collect())
}

/// Begin a transaction scoped to one tenant.
///
/// **Every query touching tenant data must go through this.** `set_config(..., true)` makes the
/// setting transaction-local, which is the detail that makes this safe under a connection pool: the
/// scope is discarded on commit or rollback, so a pooled connection can never carry one merchant's
/// tenant id into the next request.
///
/// With the setting unset, every RLS policy evaluates `tenant_id = NULL`, which is never true — so
/// forgetting to scope yields an empty result set rather than another merchant's data. The failure
/// mode is a visibly broken feature, not a silent breach.
///
/// # Errors
/// [`sqlx::Error`] if the transaction cannot be started or the scope cannot be set.
pub async fn begin_for_tenant(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let mut transaction = pool.begin().await?;

    sqlx::query("SELECT set_config('sahl.tenant_id', $1, true)")
        .bind(tenant_id.to_string())
        .execute(transaction.acquire().await?)
        .await?;

    Ok(transaction)
}

/// Confirm the connected role cannot bypass row-level security.
///
/// Called once at startup. A role with `BYPASSRLS` — or a superuser, which bypasses regardless of
/// `FORCE ROW LEVEL SECURITY` — makes every policy in the schema decorative while leaving the code
/// looking correct. That is precisely the kind of misconfiguration that survives a code review and
/// shows up as a cross-merchant data leak, so it is checked rather than assumed.
///
/// # Errors
/// [`sqlx::Error`] on query failure. Returns `Ok(false)` when the role is unsafe.
pub async fn role_respects_rls(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let (is_superuser, bypasses_rls): (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(pool)
            .await?;

    Ok(!is_superuser && !bypasses_rls)
}
