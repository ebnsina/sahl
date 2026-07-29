//! The HTTP transport.
//!
//! Blocking on purpose. The sync loop runs on its own thread precisely so it can never block a
//! sale, and blocking calls there are simpler to reason about than an async task competing with the
//! UI's runtime for the same executor.

use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use sahl_core::event::EventEnvelope;
use sahl_sync::{PullResponse, PushRequest, PushResponse, SyncRejection};
use uuid::Uuid;

use super::engine::Transport;

/// Must match `sahl_server::device::signing::SIGNING_DOMAIN`.
///
/// Duplicated rather than shared: the server crate is not a dependency of the till, and pulling it
/// in to share one constant would drag Postgres onto a merchant's counter. The end-to-end test
/// pins the two together.
const SIGNING_DOMAIN: &str = "sahl-request-v1";

const HEADER_DEVICE: &str = "x-sahl-device";
const HEADER_TIMESTAMP: &str = "x-sahl-timestamp";
const HEADER_SIGNATURE: &str = "x-sahl-signature";

const PUSH_PATH: &str = "/v1/sync/push";
const PULL_PATH: &str = "/v1/sync/pull";

/// Talks to the sync endpoints, signing every request.
#[derive(Debug)]
pub struct HttpTransport {
    client: reqwest::blocking::Client,
    base_url: String,
    device_id: Uuid,
    key: SigningKey,
}

impl HttpTransport {
    /// # Errors
    /// [`reqwest::Error`] if the HTTP client cannot be built.
    pub fn new(base_url: String, device_id: Uuid, key: SigningKey) -> Result<Self, reqwest::Error> {
        let client = reqwest::blocking::Client::builder()
            // Short by design: a till on a bad connection should fail fast and retry on the next
            // round rather than hold a thread for a minute while a cashier waits to sync.
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(5))
            .build()?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_owned(),
            device_id,
            key,
        })
    }

    /// Build the exact bytes the server will verify.
    fn signing_payload(&self, method: &str, path: &str, timestamp: i64, body: &[u8]) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        format!(
            "{SIGNING_DOMAIN}\n{}\n{}\n{}\n{}\n{}",
            method.to_ascii_uppercase(),
            path,
            self.device_id,
            timestamp,
            hex::encode(Sha256::digest(body))
        )
        .into_bytes()
    }

    fn headers(&self, method: &str, path: &str, body: &[u8]) -> [(&'static str, String); 3] {
        let timestamp = now_millis();
        let signature = self
            .key
            .sign(&self.signing_payload(method, path, timestamp, body));
        [
            (HEADER_DEVICE, self.device_id.to_string()),
            (HEADER_TIMESTAMP, timestamp.to_string()),
            (HEADER_SIGNATURE, hex::encode(signature.to_bytes())),
        ]
    }
}

impl Transport for HttpTransport {
    fn push(&mut self, events: &[EventEnvelope]) -> Result<PushResponse, SyncRejection> {
        let request = PushRequest {
            device_id: self.device_id,
            events: events.to_vec(),
        };
        let body = serde_json::to_vec(&request).map_err(|_| SyncRejection::Invalid)?;

        let mut builder = self
            .client
            .post(format!("{}{PUSH_PATH}", self.base_url))
            .header("content-type", "application/json");
        for (name, value) in self.headers("POST", PUSH_PATH, &body) {
            builder = builder.header(name, value);
        }

        // A transport failure is always retryable: the request may not have arrived at all, and the
        // server is idempotent for the case where it did.
        let response = builder
            .body(body)
            .send()
            .map_err(|_| SyncRejection::Unavailable)?;
        decode(response)
    }

    fn pull(&mut self, cursor: i64, limit: usize) -> Result<PullResponse, SyncRejection> {
        // The query string is part of the signed path, so it must be built once and reused exactly.
        let path = format!("{PULL_PATH}?cursor={cursor}&limit={limit}");

        let mut builder = self.client.get(format!("{}{path}", self.base_url));
        for (name, value) in self.headers("GET", &path, &[]) {
            builder = builder.header(name, value);
        }

        let response = builder.send().map_err(|_| SyncRejection::Unavailable)?;
        decode(response)
    }
}

/// Turn a response into a value or a rejection.
///
/// The status carries the retry decision — 503 means try again, anything else in the error range
/// does not — so it is read before the body, which on failure is a rejection rather than a payload.
fn decode<T: serde::de::DeserializeOwned>(
    response: reqwest::blocking::Response,
) -> Result<T, SyncRejection> {
    let status = response.status();
    if status.is_success() {
        return response.json::<T>().map_err(|_| SyncRejection::Invalid);
    }

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(SyncRejection::NotAuthorised);
    }
    if status.is_server_error() {
        return Err(SyncRejection::Unavailable);
    }

    // The server names the reason for a 4xx; fall back to Invalid if the body is unreadable, since
    // retrying an unexplained 4xx forever helps nobody.
    Err(response
        .json::<SyncRejection>()
        .unwrap_or(SyncRejection::Invalid))
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

    fn transport() -> HttpTransport {
        HttpTransport::new(
            "http://example.invalid/".to_owned(),
            Uuid::from_u128(0xD3),
            SigningKey::from_bytes(&[7u8; 32]),
        )
        .expect("builds")
    }

    #[test]
    fn a_trailing_slash_does_not_double_up_in_the_url() {
        assert_eq!(transport().base_url, "http://example.invalid");
    }

    #[test]
    fn the_signing_payload_matches_the_servers_shape() {
        // Six newline-delimited fields. The end-to-end test proves they agree byte for byte; this
        // catches a shape change without needing a server.
        let payload =
            transport().signing_payload("post", "/v1/sync/push", 1_753_000_000_000, b"{}");
        let text = String::from_utf8(payload).expect("utf-8");
        let lines: Vec<_> = text.split('\n').collect();

        assert_eq!(lines[0], "sahl-request-v1");
        assert_eq!(lines[1], "POST", "method is normalised");
        assert_eq!(lines[2], "/v1/sync/push");
        assert_eq!(lines[3], Uuid::from_u128(0xD3).to_string());
        assert_eq!(lines[4], "1753000000000");
        assert_eq!(lines.len(), 6);
    }

    #[test]
    fn the_pull_query_is_part_of_the_signed_path() {
        // Signing the bare path while requesting a different cursor would fail verification, so
        // the query has to be in both.
        let signed = transport().signing_payload("GET", "/v1/sync/pull?cursor=7&limit=50", 0, &[]);
        let text = String::from_utf8(signed).expect("utf-8");
        assert!(text.contains("/v1/sync/pull?cursor=7&limit=50"));
    }

    #[test]
    fn an_unreachable_server_is_retryable() {
        // The request may never have arrived, and the server is idempotent if it did.
        let mut transport = transport();
        assert_eq!(
            transport.pull(0, 10).unwrap_err(),
            SyncRejection::Unavailable
        );
    }
}
