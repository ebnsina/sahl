//! Sync endpoints.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, http::Uri};
use sahl_sync::{PullResponse, PushRequest, PushResponse, SyncRejection};

use crate::routes::AppState;
use crate::routes::auth::{self, AuthFailure};
use crate::sync;

pub const PUSH_PATH: &str = "/v1/sync/push";
pub const PULL_PATH: &str = "/v1/sync/pull";

/// Accept a batch of events.
pub async fn push(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let authenticated = match auth::authenticate(
        &state.pool,
        &headers,
        "POST",
        PUSH_PATH,
        &body,
        now_millis(),
        state.max_skew_seconds,
    )
    .await
    {
        Ok(value) => value,
        Err(failure) => return unauthorised(failure, &body),
    };

    let Ok(request) = serde_json::from_slice::<PushRequest>(&body) else {
        return rejected(SyncRejection::Invalid);
    };

    // The signature proves who sent the bytes; this proves the bytes are about that sender. Without
    // it a device could sign a batch naming another device's id.
    if request.device_id != authenticated.device.device_id {
        return rejected(SyncRejection::Invalid);
    }
    if let Err(error) = request.validate() {
        return rejected(sahl_sync::SyncRejection::from_sync_error(&error));
    }

    let mut transaction = authenticated.transaction;
    let response = match sync::push(&mut transaction, &authenticated.device, &request.events).await
    {
        Ok(value) => value,
        Err(error) => {
            let rejection = error.rejection();
            tracing::warn!(
                device = %authenticated.device.device_id,
                body = %auth::body_fingerprint(&body),
                "push refused: {error}"
            );
            return rejected(rejection);
        }
    };

    if transaction.commit().await.is_err() {
        return rejected(SyncRejection::Unavailable);
    }

    (StatusCode::OK, Json::<PushResponse>(response)).into_response()
}

/// Deliver events from the outlet's other devices.
pub async fn pull(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    // A GET has no body, so an empty one is what the client signed.
    let body = Bytes::new();
    let path_and_query = uri
        .path_and_query()
        .map_or(PULL_PATH, |value| value.as_str());

    let authenticated = match auth::authenticate(
        &state.pool,
        &headers,
        "GET",
        path_and_query,
        &body,
        now_millis(),
        state.max_skew_seconds,
    )
    .await
    {
        Ok(value) => value,
        Err(failure) => return unauthorised(failure, &body),
    };

    let query = uri.query().unwrap_or_default();
    let cursor = param(query, "cursor")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0i64);
    let limit = param(query, "limit")
        .and_then(|value| value.parse().ok())
        .unwrap_or(200usize)
        .min(500);

    let mut transaction = authenticated.transaction;
    let response = match sync::pull(&mut transaction, &authenticated.device, cursor, limit).await {
        Ok(value) => value,
        Err(error) => return rejected(error.rejection()),
    };

    if transaction.commit().await.is_err() {
        return rejected(SyncRejection::Unavailable);
    }

    (StatusCode::OK, Json::<PullResponse>(response)).into_response()
}

/// Every authentication failure looks identical from outside.
///
/// The variant is logged, never returned: distinguishing "unknown device" from "bad signature"
/// hands an attacker an oracle, and a legitimate till cannot act on the difference anyway.
fn unauthorised(failure: AuthFailure, body: &Bytes) -> Response {
    tracing::warn!(
        reason = ?failure,
        body = %auth::body_fingerprint(body),
        "request not authenticated"
    );
    (StatusCode::UNAUTHORIZED, Json(SyncRejection::NotAuthorised)).into_response()
}

fn rejected(rejection: SyncRejection) -> Response {
    let status = if rejection.is_retryable() {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    (status, Json(rejection)).into_response()
}

fn param<'q>(query: &'q str, key: &str) -> Option<&'q str> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then_some(value)
    })
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parameters_are_read_by_name() {
        assert_eq!(param("cursor=17&limit=50", "cursor"), Some("17"));
        assert_eq!(param("cursor=17&limit=50", "limit"), Some("50"));
        assert_eq!(param("cursor=17", "missing"), None);
        assert_eq!(param("", "cursor"), None);
    }

    #[test]
    fn a_retryable_rejection_maps_to_503_and_the_rest_to_422() {
        // The status is what a client's backoff keys off, so the split has to be right.
        assert!(SyncRejection::Unavailable.is_retryable());
        assert!(!SyncRejection::Forked.is_retryable());
    }
}
