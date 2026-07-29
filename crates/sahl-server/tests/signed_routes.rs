//! The sync endpoints over real HTTP, with real signatures.
//!
//! Skipped unless `SAHL_TEST_DATABASE_URL` is set. Signature verification against a stored public
//! key cannot be meaningfully faked — the point is that the wiring is right, not the crypto.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use sahl_core::Timestamp;
use sahl_core::event::{EventChain, EventEnvelope, EventHeader, EventPayload};
use sahl_server::db;
use sahl_server::device::SignedRequest;
use sahl_server::routes::{AppState, auth, router, sync as sync_routes};
use serde::Serialize;
use sqlx::{Acquire, PgPool};
use uuid::Uuid;

#[derive(Serialize)]
struct Sale {
    n: u32,
}
impl EventPayload for Sale {
    fn kind(&self) -> &'static str {
        "sale.completed"
    }
}

async fn pool() -> Option<PgPool> {
    let url = std::env::var("SAHL_TEST_DATABASE_URL").ok()?;
    db::connect(&url, 5).await.ok()
}

struct Fixture {
    base: String,
    device: Uuid,
    tenant: Uuid,
    outlet: Uuid,
    key: SigningKey,
}

/// Boot the real router on an ephemeral port with a seeded device.
async fn start(pool: &PgPool) -> Fixture {
    let (tenant, outlet, device) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let key = SigningKey::from_bytes(&[11u8; 32]);

    let mut tx = db::begin_for_tenant(pool, tenant).await.expect("scoped tx");
    sqlx::query(
        "INSERT INTO tenant (id, name, country_code, default_currency) VALUES ($1,'T','BD','BDT')",
    )
    .bind(tenant)
    .execute(tx.acquire().await.unwrap())
    .await
    .expect("tenant");
    sqlx::query(
        "INSERT INTO outlet (id, tenant_id, name, profile, timezone, currency) \
         VALUES ($1,$2,'Main','retail','Asia/Dhaka','BDT')",
    )
    .bind(outlet)
    .bind(tenant)
    .execute(tx.acquire().await.unwrap())
    .await
    .expect("outlet");
    sqlx::query(
        "INSERT INTO device (id, tenant_id, outlet_id, label, public_key) VALUES ($1,$2,$3,'Till',$4)",
    )
    .bind(device)
    .bind(tenant)
    .bind(outlet)
    .bind(key.verifying_key().to_bytes().as_slice())
    .execute(tx.acquire().await.unwrap())
    .await
    .expect("device");
    tx.commit().await.expect("commit");

    let app = router(AppState {
        pool: pool.clone(),
        max_skew_seconds: 300,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    Fixture {
        base: format!("http://127.0.0.1:{port}"),
        device,
        tenant,
        outlet,
        key,
    }
}

fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("fits")
}

/// Sign exactly as the terminal will.
fn sign(fixture: &Fixture, method: &str, path: &str, body: &[u8], at: i64) -> String {
    let request = SignedRequest {
        device_id: fixture.device,
        method,
        path,
        timestamp_millis: at,
        body,
    };
    hex::encode(fixture.key.sign(&request.signing_payload()).to_bytes())
}

async fn post(fixture: &Fixture, body: Vec<u8>, signature: String, at: i64) -> (u16, String) {
    let response = reqwest::Client::new()
        .post(format!("{}{}", fixture.base, sync_routes::PUSH_PATH))
        .header(auth::HEADER_DEVICE, fixture.device.to_string())
        .header(auth::HEADER_TIMESTAMP, at.to_string())
        .header(auth::HEADER_SIGNATURE, signature)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .expect("request");
    let status = response.status().as_u16();
    (status, response.text().await.unwrap_or_default())
}

fn sealed(fixture: &Fixture, chain: &mut EventChain, count: u32) -> Vec<EventEnvelope> {
    (0..count)
        .map(|n| {
            chain
                .append(
                    EventHeader {
                        event_id: Uuid::now_v7(),
                        tenant_id: fixture.tenant,
                        outlet_id: fixture.outlet,
                        device_id: fixture.device,
                        occurred_at: Timestamp::from_millis(1_753_000_000_000 + i64::from(n)),
                    },
                    &Sale { n },
                )
                .expect("seals")
        })
        .collect()
}

fn push_body(fixture: &Fixture, events: &[EventEnvelope]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "device_id": fixture.device,
        "events": events,
    }))
    .expect("serialises")
}

#[tokio::test]
async fn a_correctly_signed_push_is_accepted() {
    let Some(pool) = pool().await else {
        eprintln!("skipped: SAHL_TEST_DATABASE_URL not set");
        return;
    };
    let fixture = start(&pool).await;
    let mut chain = EventChain::new(fixture.device);
    let events = sealed(&fixture, &mut chain, 5);
    let body = push_body(&fixture, &events);
    let at = now_millis();

    let signature = sign(&fixture, "POST", sync_routes::PUSH_PATH, &body, at);
    let (status, text) = post(&fixture, body, signature, at).await;

    assert_eq!(status, 200, "body: {text}");
    assert!(text.contains("\"accepted\":5"), "body: {text}");
}

#[tokio::test]
async fn an_unsigned_request_is_refused() {
    let Some(pool) = pool().await else { return };
    let fixture = start(&pool).await;
    let mut chain = EventChain::new(fixture.device);
    let body = push_body(&fixture, &sealed(&fixture, &mut chain, 1));

    let response = reqwest::Client::new()
        .post(format!("{}{}", fixture.base, sync_routes::PUSH_PATH))
        .body(body)
        .send()
        .await
        .expect("request");

    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
async fn a_tampered_body_is_refused() {
    // The headline property: an intercepted batch cannot have events added or removed.
    let Some(pool) = pool().await else { return };
    let fixture = start(&pool).await;
    let mut chain = EventChain::new(fixture.device);
    let events = sealed(&fixture, &mut chain, 3);

    let honest = push_body(&fixture, &events);
    let at = now_millis();
    let signature = sign(&fixture, "POST", sync_routes::PUSH_PATH, &honest, at);

    // Same signature, one event dropped.
    let tampered = push_body(&fixture, &events[..2]);
    let (status, _) = post(&fixture, tampered, signature, at).await;

    assert_eq!(status, 401);
}

#[tokio::test]
async fn a_stale_request_is_refused() {
    let Some(pool) = pool().await else { return };
    let fixture = start(&pool).await;
    let mut chain = EventChain::new(fixture.device);
    let body = push_body(&fixture, &sealed(&fixture, &mut chain, 1));

    // Twenty minutes old, well past the 300s window.
    let at = now_millis() - 1_200_000;
    let signature = sign(&fixture, "POST", sync_routes::PUSH_PATH, &body, at);
    let (status, _) = post(&fixture, body, signature, at).await;

    assert_eq!(status, 401);
}

#[tokio::test]
async fn another_devices_signature_is_refused() {
    let Some(pool) = pool().await else { return };
    let fixture = start(&pool).await;
    let mut chain = EventChain::new(fixture.device);
    let body = push_body(&fixture, &sealed(&fixture, &mut chain, 1));
    let at = now_millis();

    let impostor = SigningKey::from_bytes(&[99u8; 32]);
    let request = SignedRequest {
        device_id: fixture.device,
        method: "POST",
        path: sync_routes::PUSH_PATH,
        timestamp_millis: at,
        body: &body,
    };
    let signature = hex::encode(impostor.sign(&request.signing_payload()).to_bytes());

    let (status, _) = post(&fixture, body, signature, at).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn a_revoked_device_is_refused_even_when_correctly_signed() {
    let Some(pool) = pool().await else { return };
    let fixture = start(&pool).await;

    let mut tx = db::begin_for_tenant(&pool, fixture.tenant)
        .await
        .expect("tx");
    sqlx::query("UPDATE device SET revoked_at = now() WHERE id = $1")
        .bind(fixture.device)
        .execute(tx.acquire().await.unwrap())
        .await
        .expect("revoke");
    tx.commit().await.expect("commit");

    let mut chain = EventChain::new(fixture.device);
    let body = push_body(&fixture, &sealed(&fixture, &mut chain, 1));
    let at = now_millis();
    let signature = sign(&fixture, "POST", sync_routes::PUSH_PATH, &body, at);

    let (status, _) = post(&fixture, body, signature, at).await;
    assert_eq!(status, 401, "revocation must beat a valid signature");
}

#[tokio::test]
async fn a_batch_naming_another_device_is_refused() {
    // The signature proves who sent the bytes; this proves the bytes are about that sender.
    let Some(pool) = pool().await else { return };
    let fixture = start(&pool).await;
    let mut chain = EventChain::new(fixture.device);
    let events = sealed(&fixture, &mut chain, 2);

    let body = serde_json::to_vec(&serde_json::json!({
        "device_id": Uuid::now_v7(),
        "events": events,
    }))
    .expect("serialises");

    let at = now_millis();
    let signature = sign(&fixture, "POST", sync_routes::PUSH_PATH, &body, at);
    let (status, _) = post(&fixture, body, signature, at).await;

    assert_eq!(status, 422);
}

#[tokio::test]
async fn a_signed_pull_returns_a_page() {
    let Some(pool) = pool().await else { return };
    let fixture = start(&pool).await;

    let path = format!("{}?cursor=0&limit=10", sync_routes::PULL_PATH);
    let at = now_millis();
    let signature = sign(&fixture, "GET", &path, &[], at);

    let response = reqwest::Client::new()
        .get(format!("{}{path}", fixture.base))
        .header(auth::HEADER_DEVICE, fixture.device.to_string())
        .header(auth::HEADER_TIMESTAMP, at.to_string())
        .header(auth::HEADER_SIGNATURE, signature)
        .send()
        .await
        .expect("request");

    assert_eq!(response.status().as_u16(), 200);
    let text = response.text().await.unwrap_or_default();
    assert!(text.contains("\"events\":[]"), "body: {text}");
}

/// Guard against the encoding drifting: the terminal builds this string independently.
#[test]
fn the_signing_payload_is_stable() {
    let request = SignedRequest {
        device_id: Uuid::from_u128(0xD3),
        method: "POST",
        path: "/v1/sync/push",
        timestamp_millis: 1_753_000_000_000,
        body: b"{}",
    };
    let payload = String::from_utf8(request.signing_payload()).expect("utf-8");
    let lines: Vec<_> = payload.split('\n').collect();

    assert_eq!(lines[0], "sahl-request-v1");
    assert_eq!(lines[1], "POST");
    assert_eq!(lines[2], "/v1/sync/push");
    assert_eq!(lines[4], "1753000000000");
    assert_eq!(
        lines.len(),
        6,
        "domain, method, path, device, timestamp, digest"
    );

    // Unused import guard for the base64 engine used elsewhere in enrollment.
    let _ = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 1]);
}
