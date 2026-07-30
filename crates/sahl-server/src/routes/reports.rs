//! What an owner reads from their phone.
//!
//! The server stores events; it does not store totals. Every figure here is projected from the log
//! on request by `sahl-core` — the same crate the till sells with — so the dashboard and the
//! terminal cannot drift apart. Caching a total would create a second copy of a number that is
//! already derivable, and a second copy is a thing that can be wrong.
//!
//! Authenticated by a dashboard token rather than a device signature. An owner on a phone has no
//! keypair, and their till PIN is four digits — see the migration for why that is not allowed
//! anywhere near a public endpoint.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, extract::Query};
use sahl_core::projection::SaleBook;
use sahl_core::sale::SaleEvent;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db;
use crate::device::enrollment::digest_token;
use crate::routes::AppState;

pub const DAY_PATH: &str = "/api/report/day";
pub const OUTLETS_PATH: &str = "/api/outlets";

/// Who is asking, and what they may see.
#[derive(Debug)]
pub struct Reader {
    pub tenant_id: Uuid,
    /// `None` means every outlet in the tenant.
    pub outlet_id: Option<Uuid>,
    pub transaction: Transaction<'static, Postgres>,
}

/// Verify a bearer token and open a tenant-scoped transaction for it.
///
/// # Errors
/// A single 401 for every failure. Distinguishing "no such token" from "revoked" would tell
/// somebody guessing tokens when they had guessed a real one.
pub async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<Reader, StatusCode> {
    let presented = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let digest = digest_token(presented);

    // The tenant is exactly what presenting the token discovers, so this lookup cannot be
    // tenant-scoped. It is a SECURITY DEFINER function taking a digest, which a caller can only
    // produce by already holding the token.
    let row =
        sqlx::query("SELECT tenant_id, outlet_id, revoked FROM dashboard_token_for_digest($1)")
            .bind(digest.as_slice())
            .fetch_optional(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .ok_or(StatusCode::UNAUTHORIZED)?;

    if row.try_get::<bool, _>("revoked").unwrap_or(true) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let tenant_id: Uuid = row
        .try_get("tenant_id")
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let outlet_id: Option<Uuid> = row.try_get("outlet_id").ok();

    let transaction = db::begin_for_tenant(&state.pool, tenant_id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    Ok(Reader {
        tenant_id,
        outlet_id,
        transaction,
    })
}

#[derive(Debug, Deserialize)]
pub struct DayQuery {
    /// Which outlet. Required when the token is tenant-wide.
    pub outlet: Option<Uuid>,
    /// Business day bounds in milliseconds, chosen by the caller because only it knows the
    /// outlet's timezone and when the shop considers a day to have started.
    pub from: Option<i64>,
    pub to: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutletRow {
    pub id: Uuid,
    pub name: String,
}

/// The outlets this token may read.
pub async fn outlets(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let mut reader = match authenticate(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return status.into_response(),
    };

    let scoped = reader.outlet_id;
    let rows = sqlx::query("SELECT id, name FROM outlet ORDER BY name")
        .fetch_all(
            match sqlx::Acquire::acquire(&mut reader.transaction).await {
                Ok(connection) => connection,
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            },
        )
        .await;

    let Ok(rows) = rows else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    let outlets: Vec<OutletRow> = rows
        .into_iter()
        .filter_map(|row| {
            let id: Uuid = row.try_get("id").ok()?;
            // A token scoped to one outlet sees one outlet, even though RLS would allow the
            // tenant's others through.
            if scoped.is_some_and(|only| only != id) {
                return None;
            }
            Some(OutletRow {
                id,
                name: row.try_get("name").ok()?,
            })
        })
        .collect();

    Json(outlets).into_response()
}

/// A day, totalled from the log.
pub async fn day(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DayQuery>,
) -> Response {
    let mut reader = match authenticate(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return status.into_response(),
    };

    // A token scoped to one outlet may only ever read that one, whatever it asks for.
    let outlet = match (reader.outlet_id, query.outlet) {
        (Some(only), Some(asked)) if only != asked => return StatusCode::FORBIDDEN.into_response(),
        (Some(only), _) => only,
        (None, Some(asked)) => asked,
        (None, None) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let from = query.from.unwrap_or(i64::MIN);
    let to = query.to.unwrap_or(i64::MAX);

    let rows = sqlx::query(
        "SELECT payload FROM event \
         WHERE outlet_id = $1 AND kind LIKE 'sale.%' AND occurred_at BETWEEN $2 AND $3 \
         ORDER BY occurred_at, device_seq",
    )
    .bind(outlet)
    .bind(from)
    .bind(to)
    .fetch_all(
        match sqlx::Acquire::acquire(&mut reader.transaction).await {
            Ok(connection) => connection,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
    )
    .await;

    let Ok(rows) = rows else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    let events: Vec<SaleEvent> = rows
        .into_iter()
        .filter_map(|row| {
            let payload: serde_json::Value = row.try_get("payload").ok()?;
            // A payload this build cannot read belongs to a newer terminal. Skipping it reports a
            // day that is short rather than refusing to report at all — and the log still has it.
            serde_json::from_value(payload).ok()
        })
        .collect();

    let Ok(book) = SaleBook::replay(&events) else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };

    let sales: Vec<&sahl_core::Sale> = book.completed().collect();
    // The currency comes from the sales themselves. The outlet's configuration lives in the event
    // log too, but a day that contains sales already knows what it was rung in.
    let Some(currency) = sales.first().map(|sale| sale.currency()) else {
        return Json(sahl_core::report::Day::empty(sahl_core::Currency::Bdt)).into_response();
    };

    match sahl_core::report::Day::of(&sales, currency) {
        Ok(day) => Json(day).into_response(),
        Err(_) => StatusCode::UNPROCESSABLE_ENTITY.into_response(),
    }
}
