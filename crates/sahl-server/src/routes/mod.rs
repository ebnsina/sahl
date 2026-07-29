//! HTTP surface.

pub mod auth;
pub mod sync;

use axum::Router;
use axum::routing::{get, post};
use sqlx::PgPool;

/// Shared handler state.
#[derive(Clone, Debug)]
pub struct AppState {
    pub pool: PgPool,
    /// Replay window for signed requests, from configuration.
    pub max_skew_seconds: i64,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(sync::PUSH_PATH, post(sync::push))
        .route(sync::PULL_PATH, get(sync::pull))
        .with_state(state)
}
