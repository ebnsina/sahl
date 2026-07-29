//! Sahl server entry point.
//!
//! Startup order is deliberate: read config, connect, migrate, then refuse to serve if the database
//! role can bypass row-level security. Each step aborts loudly rather than degrading, because every
//! one of them failing quietly produces wrong numbers in a merchant's books rather than an outage.

#![forbid(unsafe_code)]

use std::process::ExitCode;

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
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        db::run_migrations(&pool)
            .await
            .map_err(|error| format!("migration failed: {error}"))?;
        tracing::info!("migrations applied");
        return Ok(());
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

/// Drain in-flight requests on SIGINT rather than dropping a sync batch mid-write.
async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!("could not install shutdown handler: {error}");
    }
    tracing::info!("shutting down");
}
