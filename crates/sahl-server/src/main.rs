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
    tracing_subscriber::fmt()
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
    let tenant: (uuid::Uuid,) = sqlx::query_as("SELECT tenant_id FROM outlet WHERE id = $1")
        .bind(outlet)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("could not read the outlet: {error}"))?
        .ok_or_else(|| format!("no outlet {outlet}"))?;

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

    let tenant: (uuid::Uuid,) = sqlx::query_as("SELECT tenant_id FROM outlet WHERE id = $1")
        .bind(outlet)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("could not read the outlet: {error}"))?
        .ok_or_else(|| format!("no outlet {outlet}"))?;

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
