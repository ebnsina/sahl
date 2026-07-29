//! Sync ingest against a real Postgres.
//!
//! Skipped unless `SAHL_TEST_DATABASE_URL` is set — these cannot be faked. The behaviour under test
//! is transactional and RLS-scoped, and an in-memory stand-in would prove nothing about either.
//!
//! Set up: createdb sahl_test && psql sahl_test -f crates/sahl-server/migrations/0001_foundations.sql

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use sahl_core::Timestamp;
use sahl_core::event::{EventChain, EventEnvelope, EventHeader, EventPayload};
use sahl_server::db;
use sahl_server::sync::{self, DeviceRecord};
use serde::Serialize;
use sqlx::{Acquire, PgPool};
use uuid::Uuid;

#[derive(Serialize)]
struct Sale {
    n: u32,
    total_minor: i64,
}

impl EventPayload for Sale {
    fn kind(&self) -> &'static str {
        "sale.completed"
    }
}

/// Returns `None` when no test database is configured, so the suite stays green on a machine
/// without Postgres rather than failing for an unrelated reason.
async fn pool() -> Option<PgPool> {
    let url = std::env::var("SAHL_TEST_DATABASE_URL").ok()?;
    db::connect(&url, 5).await.ok()
}

/// Fresh ids every run.
///
/// Derived ids looked tidier and were wrong: a second run against the same database collided on the
/// tenant insert, so the suite passed once and failed forever after. Random ids make runs
/// independent by construction rather than by remembering to drop the database.
async fn seed(pool: &PgPool) -> (Uuid, Uuid, Uuid, Uuid) {
    let (tenant, outlet, device_a, device_b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );

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

    for device in [device_a, device_b] {
        sqlx::query(
            "INSERT INTO device (id, tenant_id, outlet_id, label, public_key) \
             VALUES ($1,$2,$3,'Till',decode(repeat('ab',32),'hex'))",
        )
        .bind(device)
        .bind(tenant)
        .bind(outlet)
        .execute(tx.acquire().await.unwrap())
        .await
        .expect("device");
    }
    tx.commit().await.expect("commit");
    (tenant, outlet, device_a, device_b)
}

/// Seal `count` sales offline, exactly as a terminal with no network would.
fn offline_sales(
    tenant: Uuid,
    outlet: Uuid,
    device: Uuid,
    count: u32,
    chain: &mut EventChain,
) -> Vec<EventEnvelope> {
    (0..count)
        .map(|n| {
            chain
                .append(
                    EventHeader {
                        event_id: Uuid::now_v7(),
                        tenant_id: tenant,
                        outlet_id: outlet,
                        device_id: device,
                        occurred_at: Timestamp::from_millis(1_753_000_000_000 + i64::from(n)),
                    },
                    &Sale {
                        n,
                        total_minor: 1_000 + i64::from(n),
                    },
                )
                .expect("seals")
        })
        .collect()
}

async fn device_record(pool: &PgPool, tenant: Uuid, device: Uuid) -> DeviceRecord {
    let mut tx = db::begin_for_tenant(pool, tenant).await.expect("scoped tx");
    let record = sync::load_device(&mut tx, device).await.expect("device");
    tx.commit().await.expect("commit");
    record
}

/// **The unplug test.** Fifty sales with no network, then reconnect.
///
/// This is the demo that closes deals, so it is also the test that must never be allowed to rot:
/// zero loss, zero duplicates, and the server's tip agreeing with the terminal's.
#[tokio::test]
async fn fifty_offline_sales_survive_a_reconnect() {
    let Some(pool) = pool().await else {
        eprintln!("skipped: SAHL_TEST_DATABASE_URL not set");
        return;
    };
    let (tenant, outlet, device, _) = seed(&pool).await;

    let mut chain = EventChain::new(device);
    let events = offline_sales(tenant, outlet, device, 50, &mut chain);

    let record = device_record(&pool, tenant, device).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    let response = sync::push(&mut tx, &record, &events).await.expect("push");
    tx.commit().await.expect("commit");

    assert_eq!(response.accepted, 50, "every sale is stored");
    assert_eq!(response.skipped, 0);
    assert_eq!(response.tip, chain.tip(), "server agrees with the terminal");

    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    let stored: (i64,) = sqlx::query_as("SELECT count(*) FROM event WHERE device_id = $1")
        .bind(device)
        .fetch_one(tx.acquire().await.unwrap())
        .await
        .expect("count");
    tx.commit().await.expect("commit");

    assert_eq!(stored.0, 50, "zero loss, zero duplicates");
}

/// A push whose response was lost, retried. Must succeed as a no-op, not fail.
#[tokio::test]
async fn a_lost_ack_is_safe_to_retry() {
    let Some(pool) = pool().await else { return };
    let (tenant, outlet, device, _) = seed(&pool).await;

    let mut chain = EventChain::new(device);
    let events = offline_sales(tenant, outlet, device, 10, &mut chain);

    let record = device_record(&pool, tenant, device).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    sync::push(&mut tx, &record, &events)
        .await
        .expect("first push");
    tx.commit().await.expect("commit");

    // The terminal never saw the response, so it sends the identical batch again.
    let record = device_record(&pool, tenant, device).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    let retry = sync::push(&mut tx, &record, &events).await.expect("retry");
    tx.commit().await.expect("commit");

    assert_eq!(retry.accepted, 0, "nothing new");
    assert_eq!(retry.skipped, 10);
    assert_eq!(retry.tip, chain.tip());

    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    let stored: (i64,) = sqlx::query_as("SELECT count(*) FROM event WHERE device_id = $1")
        .bind(device)
        .fetch_one(tx.acquire().await.unwrap())
        .await
        .expect("count");
    tx.commit().await.expect("commit");

    assert_eq!(stored.0, 10, "a retry must not duplicate");
}

/// The terminal kept selling while the ack was in flight.
#[tokio::test]
async fn a_partial_retry_stores_only_the_new_events() {
    let Some(pool) = pool().await else { return };
    let (tenant, outlet, device, _) = seed(&pool).await;

    let mut chain = EventChain::new(device);
    let first = offline_sales(tenant, outlet, device, 5, &mut chain);

    let record = device_record(&pool, tenant, device).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    sync::push(&mut tx, &record, &first).await.expect("push");
    tx.commit().await.expect("commit");

    let mut combined = first;
    combined.extend(offline_sales(tenant, outlet, device, 3, &mut chain));

    let record = device_record(&pool, tenant, device).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    let response = sync::push(&mut tx, &record, &combined).await.expect("push");
    tx.commit().await.expect("commit");

    assert_eq!(response.skipped, 5);
    assert_eq!(response.accepted, 3);
    assert_eq!(response.tip.device_seq, 8);
}

/// Two tills in one shop. Each pulls the other's sales, never its own.
#[tokio::test]
async fn a_second_till_pulls_the_first_tills_sales() {
    let Some(pool) = pool().await else { return };
    let (tenant, outlet, device_a, device_b) = seed(&pool).await;

    let mut chain_a = EventChain::new(device_a);
    let sales_a = offline_sales(tenant, outlet, device_a, 6, &mut chain_a);
    let record_a = device_record(&pool, tenant, device_a).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    sync::push(&mut tx, &record_a, &sales_a)
        .await
        .expect("push a");
    tx.commit().await.expect("commit");

    let record_b = device_record(&pool, tenant, device_b).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    let page = sync::pull(&mut tx, &record_b, 0, 100)
        .await
        .expect("pull b");
    tx.commit().await.expect("commit");

    assert_eq!(page.events.len(), 6, "B sees A's sales");
    assert!(!page.has_more);
    assert!(
        page.events.iter().all(|event| event.device_id == device_a),
        "and never its own"
    );

    // A pulling back sees nothing — echoing its own events would double a busy shop's traffic.
    let record_a = device_record(&pool, tenant, device_a).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    let own = sync::pull(&mut tx, &record_a, 0, 100)
        .await
        .expect("pull a");
    tx.commit().await.expect("commit");

    assert!(own.events.is_empty());
}

/// Pulling drains in pages without losing or repeating an event.
#[tokio::test]
async fn pulling_pages_covers_everything_exactly_once() {
    let Some(pool) = pool().await else { return };
    let (tenant, outlet, device_a, device_b) = seed(&pool).await;

    let mut chain_a = EventChain::new(device_a);
    let sales = offline_sales(tenant, outlet, device_a, 25, &mut chain_a);
    let record_a = device_record(&pool, tenant, device_a).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    sync::push(&mut tx, &record_a, &sales).await.expect("push");
    tx.commit().await.expect("commit");

    let record_b = device_record(&pool, tenant, device_b).await;
    let mut seen = Vec::new();
    let mut cursor = 0i64;
    loop {
        let mut tx = db::begin_for_tenant(&pool, tenant)
            .await
            .expect("scoped tx");
        let page = sync::pull(&mut tx, &record_b, cursor, 10)
            .await
            .expect("pull");
        tx.commit().await.expect("commit");

        seen.extend(page.events.iter().map(|event| event.event_id));
        cursor = page.cursor;
        if !page.has_more {
            break;
        }
    }

    assert_eq!(seen.len(), 25);
    let mut unique = seen.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 25, "no event delivered twice");
}

/// A revoked device is refused even with a perfectly valid batch. This is what stops a stolen till.
#[tokio::test]
async fn a_revoked_device_cannot_push() {
    let Some(pool) = pool().await else { return };
    let (tenant, outlet, device, _) = seed(&pool).await;

    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    sqlx::query("UPDATE device SET revoked_at = now() WHERE id = $1")
        .bind(device)
        .execute(tx.acquire().await.unwrap())
        .await
        .expect("revoke");
    tx.commit().await.expect("commit");

    let mut chain = EventChain::new(device);
    let events = offline_sales(tenant, outlet, device, 3, &mut chain);

    let record = device_record(&pool, tenant, device).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    let result = sync::push(&mut tx, &record, &events).await;

    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().rejection(),
        sahl_sync::SyncRejection::NotAuthorised
    );
}

/// A batch with a hole is refused whole — no partial commit.
#[tokio::test]
async fn a_gap_is_refused_without_storing_anything() {
    let Some(pool) = pool().await else { return };
    let (tenant, outlet, device, _) = seed(&pool).await;

    let mut chain = EventChain::new(device);
    let events = offline_sales(tenant, outlet, device, 10, &mut chain);

    let record = device_record(&pool, tenant, device).await;
    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    let result = sync::push(&mut tx, &record, &events[4..]).await;
    drop(tx);

    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().rejection(),
        sahl_sync::SyncRejection::Gap
    );

    let mut tx = db::begin_for_tenant(&pool, tenant)
        .await
        .expect("scoped tx");
    let stored: (i64,) = sqlx::query_as("SELECT count(*) FROM event WHERE device_id = $1")
        .bind(device)
        .fetch_one(tx.acquire().await.unwrap())
        .await
        .expect("count");
    tx.commit().await.expect("commit");

    assert_eq!(stored.0, 0, "nothing partially committed");
}
