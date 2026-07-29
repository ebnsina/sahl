//! Enrollment over real HTTP.
//!
//! Skipped unless `SAHL_TEST_DATABASE_URL` is set.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_precision_loss
)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::SigningKey;
use sahl_server::db;
use sahl_server::device::{MintedToken, mint_token};
use sahl_server::routes::{AppState, enroll as enroll_routes, router};
use sqlx::{Acquire, PgPool};
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("SAHL_TEST_DATABASE_URL").ok()?;
    db::connect(&url, 5).await.ok()
}

struct Shop {
    base: String,
    tenant: Uuid,
    outlet: Uuid,
    pool: PgPool,
}

/// Seed a merchant and start the real server. No devices — that is what enrollment is for.
async fn open_shop(pool: &PgPool) -> Shop {
    let (tenant, outlet) = (Uuid::now_v7(), Uuid::now_v7());

    let mut tx = db::begin_for_tenant(pool, tenant).await.expect("tx");
    sqlx::query(
        "INSERT INTO tenant (id, name, country_code, default_currency) VALUES ($1,'Karim','BD','BDT')",
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

    Shop {
        base: format!("http://127.0.0.1:{port}"),
        tenant,
        outlet,
        pool: pool.clone(),
    }
}

/// Issue a token, as the owner's dashboard would. Only the digest is stored.
async fn issue_token(shop: &Shop, ttl_seconds: i64) -> MintedToken {
    let minted = mint_token().expect("entropy");
    let mut tx = db::begin_for_tenant(&shop.pool, shop.tenant)
        .await
        .expect("tx");
    sqlx::query(
        "INSERT INTO enrollment_token (id, tenant_id, outlet_id, token_hash, expires_at) \
         VALUES ($1,$2,$3,$4, now() + make_interval(secs => $5))",
    )
    .bind(Uuid::now_v7())
    .bind(shop.tenant)
    .bind(shop.outlet)
    .bind(minted.digest.as_slice())
    .bind(ttl_seconds as f64)
    .execute(tx.acquire().await.unwrap())
    .await
    .expect("token");
    tx.commit().await.expect("commit");
    minted
}

async fn post_enroll(shop: &Shop, token: &str, public_key: &str, label: &str) -> (u16, String) {
    let response = reqwest::Client::new()
        .post(format!("{}{}", shop.base, enroll_routes::ENROLL_PATH))
        .json(&serde_json::json!({
            "token": token,
            "public_key": public_key,
            "label": label,
        }))
        .send()
        .await
        .expect("request");
    let status = response.status().as_u16();
    (status, response.text().await.unwrap_or_default())
}

fn a_public_key(seed: u8) -> String {
    URL_SAFE_NO_PAD.encode(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes(),
    )
}

#[tokio::test]
async fn a_valid_token_enrolls_a_device() {
    let Some(pool) = pool().await else {
        eprintln!("skipped: SAHL_TEST_DATABASE_URL not set");
        return;
    };
    let shop = open_shop(&pool).await;
    let token = issue_token(&shop, 900).await;

    let (status, body) = post_enroll(&shop, &token.plaintext, &a_public_key(5), "Till 1").await;

    assert_eq!(status, 201, "body: {body}");
    assert!(body.contains(&shop.outlet.to_string()), "body: {body}");

    let mut tx = db::begin_for_tenant(&pool, shop.tenant).await.expect("tx");
    let devices: (i64,) = sqlx::query_as("SELECT count(*) FROM device WHERE outlet_id = $1")
        .bind(shop.outlet)
        .fetch_one(tx.acquire().await.unwrap())
        .await
        .expect("count");
    tx.commit().await.expect("commit");
    assert_eq!(devices.0, 1);
}

#[tokio::test]
async fn a_token_cannot_enrol_a_second_device() {
    // Single-use is what stops one leaked token enrolling a fleet of rogue terminals.
    let Some(pool) = pool().await else { return };
    let shop = open_shop(&pool).await;
    let token = issue_token(&shop, 900).await;

    let (first, _) = post_enroll(&shop, &token.plaintext, &a_public_key(5), "Till 1").await;
    let (second, _) = post_enroll(&shop, &token.plaintext, &a_public_key(6), "Till 2").await;

    assert_eq!(first, 201);
    assert_eq!(second, 403);

    let mut tx = db::begin_for_tenant(&pool, shop.tenant).await.expect("tx");
    let devices: (i64,) = sqlx::query_as("SELECT count(*) FROM device WHERE outlet_id = $1")
        .bind(shop.outlet)
        .fetch_one(tx.acquire().await.unwrap())
        .await
        .expect("count");
    tx.commit().await.expect("commit");
    assert_eq!(devices.0, 1, "only the first redemption created a device");
}

#[tokio::test]
async fn an_expired_token_is_refused() {
    let Some(pool) = pool().await else { return };
    let shop = open_shop(&pool).await;
    let token = issue_token(&shop, -60).await;

    let (status, _) = post_enroll(&shop, &token.plaintext, &a_public_key(5), "Till 1").await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn an_unknown_token_is_refused() {
    let Some(pool) = pool().await else { return };
    let shop = open_shop(&pool).await;

    let (status, _) = post_enroll(&shop, "not-a-real-token", &a_public_key(5), "Till 1").await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn a_weak_public_key_is_refused() {
    // An all-zero key makes signatures forgeable by anyone; such a device must never exist.
    let Some(pool) = pool().await else { return };
    let shop = open_shop(&pool).await;
    let token = issue_token(&shop, 900).await;

    let weak = URL_SAFE_NO_PAD.encode([0u8; 32]);
    let (status, _) = post_enroll(&shop, &token.plaintext, &weak, "Till 1").await;
    assert_eq!(status, 403);

    // And the token survives, so an operator can retry with a working terminal.
    let (retry, _) = post_enroll(&shop, &token.plaintext, &a_public_key(5), "Till 1").await;
    assert_eq!(retry, 201, "a refused key must not burn the token");
}

#[tokio::test]
async fn every_refusal_looks_identical() {
    // Naming the reason would let someone probe which tokens exist.
    let Some(pool) = pool().await else { return };
    let shop = open_shop(&pool).await;
    let expired = issue_token(&shop, -60).await;

    let (_, unknown_body) = post_enroll(&shop, "nope", &a_public_key(5), "Till").await;
    let (_, expired_body) = post_enroll(&shop, &expired.plaintext, &a_public_key(5), "Till").await;
    let (_, bad_key_body) = post_enroll(&shop, "nope", "!!!", "Till").await;

    assert_eq!(unknown_body, expired_body);
    assert_eq!(expired_body, bad_key_body);
}

#[tokio::test]
async fn an_enrolled_device_can_immediately_sync() {
    // The point of enrolling: the key the terminal generated actually authenticates.
    let Some(pool) = pool().await else { return };
    let shop = open_shop(&pool).await;
    let token = issue_token(&shop, 900).await;
    let key = SigningKey::from_bytes(&[42u8; 32]);
    let public = URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes());

    let (status, body) = post_enroll(&shop, &token.plaintext, &public, "Till 1").await;
    assert_eq!(status, 201);

    let reply: serde_json::Value = serde_json::from_str(&body).expect("json");
    let device_id: Uuid = reply["device_id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");

    // Sign an empty pull exactly as the terminal does.
    use ed25519_dalek::Signer;
    use sha2::{Digest, Sha256};
    let path = "/v1/sync/pull?cursor=0&limit=10";
    let at = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("fits");
    let payload = format!(
        "sahl-request-v1\nGET\n{path}\n{device_id}\n{at}\n{}",
        hex::encode(Sha256::digest(b""))
    );
    let signature = hex::encode(key.sign(payload.as_bytes()).to_bytes());

    let response = reqwest::Client::new()
        .get(format!("{}{path}", shop.base))
        .header("x-sahl-device", device_id.to_string())
        .header("x-sahl-timestamp", at.to_string())
        .header("x-sahl-signature", signature)
        .send()
        .await
        .expect("request");

    assert_eq!(
        response.status().as_u16(),
        200,
        "the freshly enrolled key authenticates"
    );
}
