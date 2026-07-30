//! Sahl server entry point.
//!
//! Startup order is deliberate: read config, connect, migrate, then refuse to serve if the database
//! role can bypass row-level security. Each step aborts loudly rather than degrading, because every
//! one of them failing quietly produces wrong numbers in a merchant's books rather than an outage.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use sahl_core::staff::{Role, pin};
use sahl_server::config::Config;
use sahl_server::db;
use sahl_server::routes::{AppState, router};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    // Logs to stderr, data to stdout. The token-issuing subcommands print a credential nobody can
    // recover later, and a log line interleaved into that output makes it unscriptable — which is
    // how a token gets pasted with a timestamp glued to the front of it.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            // Deliberately an error log plus a non-zero exit, not a panic: an operator reading a
            // container log should see a sentence, not a stack trace.
            tracing::error!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let config = Config::from_env().map_err(|error| format!("configuration error: {error}"))?;

    let pool = db::connect(&config.database_url, config.database_max_connections)
        .await
        .map_err(|error| format!("database unavailable: {error}"))?;

    // `sahl-server migrate` is a separate, privileged deploy step. Normal startup only verifies.
    match std::env::args().nth(1).as_deref() {
        Some("migrate") => {
            db::run_migrations(&pool)
                .await
                .map_err(|error| format!("migration failed: {error}"))?;
            tracing::info!("migrations applied");
            return Ok(());
        }
        Some("issue-token") => return issue_token(&pool).await,
        Some("add-staff") => return add_staff(&pool).await,
        Some("issue-dashboard-token") => return issue_dashboard_token(&pool).await,
        _ => {}
    }

    let pending = db::pending_migrations(&pool)
        .await
        .map_err(|error| format!("could not read migration state: {error}"))?;
    if !pending.is_empty() {
        return Err(format!(
            "database schema is behind this binary; migrations {pending:?} are not applied. \
             Run `sahl-server migrate` with a DDL-capable role first."
        ));
    }

    if !db::role_respects_rls(&pool)
        .await
        .map_err(|error| format!("could not inspect database role: {error}"))?
    {
        return Err(
            "the configured database role is a superuser or holds BYPASSRLS, which disables every \
             row-level security policy in the schema. Connect as a NOSUPERUSER NOBYPASSRLS role — \
             see crates/sahl-server/migrations/README.md."
                .to_owned(),
        );
    }

    let skew = i64::try_from(config.signature_max_skew.as_secs()).unwrap_or(300);
    let app = router(AppState {
        pool,
        max_skew_seconds: skew,
    });

    let listener = tokio::net::TcpListener::bind(config.bind_address)
        .await
        .map_err(|error| format!("could not bind {}: {error}", config.bind_address))?;

    tracing::info!(address = %config.bind_address, "sahl-server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| format!("server error: {error}"))
}

/// Mint an enrollment token and print it once.
///
/// An interim path until owners can issue tokens from the dashboard, which needs staff
/// authentication that lands in P3. Building half an auth system to avoid a CLI would be the worse
/// trade, and this is the same code path the dashboard will use.
///
/// Usage: `sahl-server issue-token <outlet-id> [ttl-seconds]`
async fn issue_token(pool: &sqlx::PgPool) -> Result<(), String> {
    let outlet: uuid::Uuid = std::env::args()
        .nth(2)
        .ok_or("usage: sahl-server issue-token <outlet-id> [ttl-seconds]")?
        .parse()
        .map_err(|_| "outlet id must be a UUID".to_owned())?;

    let ttl_seconds: i64 = std::env::args()
        .nth(3)
        .map_or(Ok(900), |value| value.parse())
        .map_err(|_| "ttl-seconds must be a number".to_owned())?;

    // The outlet's tenant has to be read before the transaction can be scoped, and the device
    // lookup already solves exactly this problem for enrollment.
    // Through the lookup function, not a plain SELECT. Row-level security is FORCEd on `outlet`,
    // and nothing has scoped this connection to a tenant yet — the tenant is what is being looked
    // up. A direct read returns no rows as the runtime role and every row as a superuser, so it
    // would have worked in development and failed in production.
    let tenant: (uuid::Uuid,) = {
        let found: (Option<uuid::Uuid>,) = sqlx::query_as("SELECT outlet_tenant($1)")
            .bind(outlet)
            .fetch_optional(pool)
            .await
            .map_err(|error| format!("could not read the outlet: {error}"))?
            .ok_or_else(|| format!("no outlet {outlet}"))?;
        (found.0.ok_or_else(|| format!("no outlet {outlet}"))?,)
    };

    let minted = sahl_server::device::mint_token()
        .map_err(|error| format!("could not mint a token: {error}"))?;

    let mut transaction = db::begin_for_tenant(pool, tenant.0)
        .await
        .map_err(|error| format!("could not open a transaction: {error}"))?;

    sqlx::query(
        "INSERT INTO enrollment_token (id, tenant_id, outlet_id, token_hash, expires_at) \
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(tenant.0)
    .bind(outlet)
    .bind(minted.digest.as_slice())
    .bind(f64::from(i32::try_from(ttl_seconds).unwrap_or(900)))
    .execute(
        sqlx::Acquire::acquire(&mut transaction)
            .await
            .map_err(|error| format!("could not acquire a connection: {error}"))?,
    )
    .await
    .map_err(|error| format!("could not store the token: {error}"))?;

    transaction
        .commit()
        .await
        .map_err(|error| format!("could not commit: {error}"))?;

    // Printed to stdout and never logged. This is the only moment the plaintext exists — the
    // database holds a digest, so a lost token is reissued rather than recovered.
    println!("{}", minted.plaintext);
    eprintln!("valid for {ttl_seconds}s, single use, for outlet {outlet}");
    Ok(())
}

/// Mint a long-lived token an owner uses to read their shop from a phone.
///
/// Not their till PIN: four digits on a public endpoint is guessable, and the only thing that made
/// a PIN reasonable was that guessing it required standing at the counter.
///
/// Usage: `sahl-server issue-dashboard-token <outlet-id|tenant:TENANT-ID> <label>`
async fn issue_dashboard_token(pool: &sqlx::PgPool) -> Result<(), String> {
    const USAGE: &str =
        "usage: sahl-server issue-dashboard-token <outlet-id|tenant:TENANT-ID> <label>";

    let scope = std::env::args().nth(2).ok_or(USAGE)?;
    let label = std::env::args().nth(3).ok_or(USAGE)?;
    if label.trim().is_empty() {
        return Err("a token needs a label, so the right one can be revoked later".to_owned());
    }

    // Tenant-wide is spelled differently on purpose. A token that reads every outlet in a chain is
    // a bigger thing to hand out than one that reads a single shop, and it should not be one
    // mistyped argument away.
    let (tenant, outlet) = if let Some(raw) = scope.strip_prefix("tenant:") {
        let tenant: uuid::Uuid = raw
            .parse()
            .map_err(|_| "tenant id must be a UUID".to_owned())?;
        (tenant, None)
    } else {
        let outlet: uuid::Uuid = scope
            .parse()
            .map_err(|_| "outlet id must be a UUID".to_owned())?;
        let found: (Option<uuid::Uuid>,) = sqlx::query_as("SELECT outlet_tenant($1)")
            .bind(outlet)
            .fetch_optional(pool)
            .await
            .map_err(|error| format!("could not read the outlet: {error}"))?
            .ok_or_else(|| format!("no outlet {outlet}"))?;
        (
            found.0.ok_or_else(|| format!("no outlet {outlet}"))?,
            Some(outlet),
        )
    };

    let minted = sahl_server::device::mint_token()
        .map_err(|error| format!("could not mint a token: {error}"))?;

    let mut transaction = db::begin_for_tenant(pool, tenant)
        .await
        .map_err(|error| format!("could not open a transaction: {error}"))?;

    sqlx::query(
        "INSERT INTO dashboard_token (id, tenant_id, outlet_id, label, token_hash) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(tenant)
    .bind(outlet)
    .bind(label.trim())
    .bind(minted.digest.as_slice())
    .execute(
        sqlx::Acquire::acquire(&mut transaction)
            .await
            .map_err(|error| format!("could not acquire a connection: {error}"))?,
    )
    .await
    .map_err(|error| format!("could not store the token: {error}"))?;

    transaction
        .commit()
        .await
        .map_err(|error| format!("could not commit: {error}"))?;

    // stdout, never the log. This is the only moment the plaintext exists — the database holds a
    // digest, so a lost token is reissued rather than recovered.
    println!("{}", minted.plaintext);
    match outlet {
        Some(id) => eprintln!("reads outlet {id}, revoke by label {label:?}"),
        None => eprintln!("reads EVERY outlet in tenant {tenant}, revoke by label {label:?}"),
    }
    Ok(())
}

/// Create a staff user with a PIN.
///
/// The PIN is read from `SAHL_STAFF_PIN` rather than an argument, because an argument is visible in
/// `ps` and lands in shell history.
///
/// Usage: `SAHL_STAFF_PIN=… sahl-server add-staff <outlet-id> <role> <name>`
async fn add_staff(pool: &sqlx::PgPool) -> Result<(), String> {
    const USAGE: &str = "usage: SAHL_STAFF_PIN=… sahl-server add-staff <outlet-id> <role> <name>";

    let outlet: uuid::Uuid = std::env::args()
        .nth(2)
        .ok_or(USAGE)?
        .parse()
        .map_err(|_| "outlet id must be a UUID".to_owned())?;

    let role: Role = match std::env::args().nth(3).ok_or(USAGE)?.as_str() {
        "owner" => Role::Owner,
        "manager" => Role::Manager,
        "cashier" => Role::Cashier,
        other => {
            return Err(format!(
                "unknown role {other:?}; expected owner|manager|cashier"
            ));
        }
    };

    let name = std::env::args().nth(4).ok_or(USAGE)?;
    if name.trim().is_empty() {
        return Err("a staff name cannot be blank".to_owned());
    }

    let secret = std::env::var("SAHL_STAFF_PIN")
        .map_err(|_| "set SAHL_STAFF_PIN to the staff member's PIN".to_owned())?;

    let salt = SaltString::generate(&mut OsRng);
    let pin_hash = pin::hash(&secret, &salt).map_err(|error| error.to_string())?;

    // Through the lookup function, not a plain SELECT. Row-level security is FORCEd on `outlet`,
    // and nothing has scoped this connection to a tenant yet — the tenant is what is being looked
    // up. A direct read returns no rows as the runtime role and every row as a superuser, so it
    // would have worked in development and failed in production.
    let tenant: (uuid::Uuid,) = {
        let found: (Option<uuid::Uuid>,) = sqlx::query_as("SELECT outlet_tenant($1)")
            .bind(outlet)
            .fetch_optional(pool)
            .await
            .map_err(|error| format!("could not read the outlet: {error}"))?
            .ok_or_else(|| format!("no outlet {outlet}"))?;
        (found.0.ok_or_else(|| format!("no outlet {outlet}"))?,)
    };

    // An owner is tenant-wide; a manager or cashier belongs to the outlet they were created for.
    let scope = (!matches!(role, Role::Owner)).then_some(outlet);

    let id = uuid::Uuid::now_v7();
    let mut transaction = db::begin_for_tenant(pool, tenant.0)
        .await
        .map_err(|error| format!("could not open a transaction: {error}"))?;

    sqlx::query(
        "INSERT INTO app_user (id, tenant_id, outlet_id, name, role, pin_hash) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(tenant.0)
    .bind(scope)
    .bind(name.trim())
    .bind(role.label())
    .bind(&pin_hash)
    .execute(
        sqlx::Acquire::acquire(&mut transaction)
            .await
            .map_err(|error| format!("could not acquire a connection: {error}"))?,
    )
    .await
    .map_err(|error| format!("could not store the staff member: {error}"))?;

    transaction
        .commit()
        .await
        .map_err(|error| format!("could not commit: {error}"))?;

    // The id goes to stdout; the PIN is never echoed, logged, or repeated back.
    println!("{id}");
    eprintln!(
        "{} added as {} for outlet {outlet}",
        name.trim(),
        role.label()
    );
    Ok(())
}

/// Drain in-flight requests on SIGINT rather than dropping a sync batch mid-write.
async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!("could not install shutdown handler: {error}");
    }
    tracing::info!("shutting down");
}
