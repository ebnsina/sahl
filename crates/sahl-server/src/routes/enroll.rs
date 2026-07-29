//! Device enrollment.
//!
//! The one unauthenticated endpoint, because a device being enrolled has no key yet. What stands in
//! for authentication is the token: single-use, short-lived, stored only as a digest, and issued by
//! an owner who is already signed in.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::Acquire;
use uuid::Uuid;

use crate::db;
use crate::device::{StoredToken, check_token_usable, digest_token, parse_public_key};
use crate::routes::AppState;

pub const ENROLL_PATH: &str = "/v1/devices/enroll";

#[derive(Debug, Deserialize)]
pub struct EnrollRequest {
    /// The plaintext token, as typed or scanned by the operator.
    pub token: String,
    /// Base64url Ed25519 public key. The private half never leaves the terminal.
    pub public_key: String,
    /// What the shop calls this till.
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct EnrollResponse {
    pub device_id: Uuid,
    pub tenant_id: Uuid,
    pub outlet_id: Uuid,
}

/// Enrollment never explains itself.
///
/// A single opaque failure for every case — bad token, expired, already used, malformed key. Naming
/// the reason would let someone probe which tokens exist, and an operator holding a genuine token
/// gets no benefit from the distinction.
#[derive(Debug, Serialize)]
pub struct EnrollRefused {
    pub error: &'static str,
}

const REFUSED: EnrollRefused = EnrollRefused {
    error: "enrollment_refused",
};

/// Bind a device's public key to an outlet, consuming the token.
pub async fn enroll(State(state): State<AppState>, Json(request): Json<EnrollRequest>) -> Response {
    let Ok(public_key) = parse_public_key(&request.public_key) else {
        return refused();
    };
    if request.label.trim().is_empty() {
        return refused();
    }

    let digest = digest_token(&request.token);

    // Looking up an enrollment token is the second query that cannot be tenant-scoped: the tenant is
    // what the token is being redeemed to discover. Like device_tenant, it goes through a narrow
    // SECURITY DEFINER function rather than opening up the table.
    let row: Option<(Uuid, Uuid, Uuid, i64, bool)> =
        match sqlx::query_as("SELECT * FROM enrollment_token_for_digest($1)")
            .bind(digest.as_slice())
            .fetch_optional(&state.pool)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                tracing::error!("enrollment lookup failed: {error}");
                return unavailable();
            }
        };

    let Some((token_id, tenant_id, outlet_id, expires_at_millis, consumed)) = row else {
        return refused();
    };

    let stored = StoredToken {
        expires_at_millis,
        consumed,
    };
    if check_token_usable(&stored, now_millis()).is_err() {
        return refused();
    }

    let Ok(mut transaction) = db::begin_for_tenant(&state.pool, tenant_id).await else {
        return unavailable();
    };

    let device_id = Uuid::now_v7();

    // Consume the token first, and conditionally. Two terminals redeeming the same token at once
    // both pass the check above; only one wins this UPDATE, and the loser enrolls nothing.
    let consumed_now: Result<u64, _> = sqlx::query(
        "UPDATE enrollment_token SET consumed_at = now(), consumed_by = $1 \
         WHERE id = $2 AND consumed_at IS NULL",
    )
    .bind(device_id)
    .bind(token_id)
    .execute(match transaction.acquire().await {
        Ok(connection) => connection,
        Err(_) => return unavailable(),
    })
    .await
    .map(|result| result.rows_affected());

    match consumed_now {
        Ok(1) => {}
        Ok(_) => return refused(),
        Err(error) => {
            tracing::error!("could not consume enrollment token: {error}");
            return unavailable();
        }
    }

    let inserted = sqlx::query(
        "INSERT INTO device (id, tenant_id, outlet_id, label, public_key) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(device_id)
    .bind(tenant_id)
    .bind(outlet_id)
    .bind(request.label.trim())
    .bind(public_key.as_slice())
    .execute(match transaction.acquire().await {
        Ok(connection) => connection,
        Err(_) => return unavailable(),
    })
    .await;

    if let Err(error) = inserted {
        tracing::error!("could not create device: {error}");
        return unavailable();
    }

    if transaction.commit().await.is_err() {
        return unavailable();
    }

    tracing::info!(device = %device_id, outlet = %outlet_id, "device enrolled");
    (
        StatusCode::CREATED,
        Json(EnrollResponse {
            device_id,
            tenant_id,
            outlet_id,
        }),
    )
        .into_response()
}

fn refused() -> Response {
    (StatusCode::FORBIDDEN, Json(REFUSED)).into_response()
}

fn unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(REFUSED)).into_response()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
