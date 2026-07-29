//! Authenticating a terminal's request.
//!
//! Verification happens against the **raw body bytes**, before any parsing. Parsing first and
//! re-serialising to check a signature is a classic way to sign one thing and act on another.

use axum::body::Bytes;
use axum::http::HeaderMap;
use sahl_core::event::EventHash;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db;
use crate::device::{DeviceCredentials, SignedRequest, verify_request};
use crate::sync::{self, DeviceRecord};

/// Header carrying the device's id.
pub const HEADER_DEVICE: &str = "x-sahl-device";
/// Header carrying the request timestamp, in milliseconds.
pub const HEADER_TIMESTAMP: &str = "x-sahl-timestamp";
/// Header carrying the hex-encoded Ed25519 signature.
pub const HEADER_SIGNATURE: &str = "x-sahl-signature";

/// Why a request was not authenticated.
///
/// Callers must collapse every variant into one opaque 401. Telling a client *which* check failed
/// helps an attacker enumerate; it does not help a cashier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailure {
    MissingHeaders,
    MalformedHeaders,
    UnknownDevice,
    Revoked,
    BadSignature,
    Unavailable,
}

/// An authenticated request: the device, and a transaction already scoped to its tenant.
#[derive(Debug)]
pub struct Authenticated {
    pub device: DeviceRecord,
    pub transaction: Transaction<'static, Postgres>,
}

/// Verify a signed request and open a tenant-scoped transaction for it.
///
/// # Errors
/// [`AuthFailure`] — collapse to a single 401 at the boundary.
pub async fn authenticate(
    pool: &PgPool,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    body: &Bytes,
    now_millis: i64,
    max_skew_seconds: i64,
) -> Result<Authenticated, AuthFailure> {
    let device_id = header(headers, HEADER_DEVICE)?
        .parse::<Uuid>()
        .map_err(|_| AuthFailure::MalformedHeaders)?;
    let timestamp = header(headers, HEADER_TIMESTAMP)?
        .parse::<i64>()
        .map_err(|_| AuthFailure::MalformedHeaders)?;
    let signature = hex::decode(header(headers, HEADER_SIGNATURE)?)
        .map_err(|_| AuthFailure::MalformedHeaders)?;

    // The device row lives behind RLS, so the transaction must be scoped before it can be read —
    // and the tenant is only known *from* that row. The lookup therefore runs unscoped and reads a
    // single row by primary key, then the transaction is reopened scoped to what it found.
    let tenant = tenant_of(pool, device_id).await?;

    let mut transaction = db::begin_for_tenant(pool, tenant)
        .await
        .map_err(|_| AuthFailure::Unavailable)?;

    let device = sync::load_device(&mut transaction, device_id)
        .await
        .map_err(|_| AuthFailure::UnknownDevice)?;

    if device.revoked {
        return Err(AuthFailure::Revoked);
    }

    let request = SignedRequest {
        device_id,
        method,
        path,
        timestamp_millis: timestamp,
        body,
    };
    let credentials = DeviceCredentials {
        device_id,
        public_key: device.public_key,
        revoked: device.revoked,
    };

    verify_request(
        &request,
        &credentials,
        &signature,
        now_millis,
        max_skew_seconds,
    )
    .map_err(|_| AuthFailure::BadSignature)?;

    Ok(Authenticated {
        device,
        transaction,
    })
}

/// Read a device's tenant so the request can be scoped.
///
/// A `SECURITY DEFINER` function rather than a plain select, because the `device` table is behind
/// RLS and there is no tenant to scope by until this returns. It exposes exactly one column for one
/// primary key, so it widens nothing else.
async fn tenant_of(pool: &PgPool, device_id: Uuid) -> Result<Uuid, AuthFailure> {
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT device_tenant($1)")
        .bind(device_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| AuthFailure::Unavailable)?;

    row.map(|(tenant,)| tenant)
        .ok_or(AuthFailure::UnknownDevice)
}

fn header<'h>(headers: &'h HeaderMap, name: &str) -> Result<&'h str, AuthFailure> {
    headers
        .get(name)
        .ok_or(AuthFailure::MissingHeaders)?
        .to_str()
        .map_err(|_| AuthFailure::MalformedHeaders)
}

/// Hash a body for logging without recording its contents.
///
/// Sync bodies are a merchant's sales. A digest is enough to correlate a client complaint with a
/// server log line; the payload itself has no business in one.
#[must_use]
pub fn body_fingerprint(body: &Bytes) -> String {
    EventHash::digest(body)
        .to_hex()
        .get(..12)
        .unwrap_or("")
        .to_owned()
}
