//! The unplug test, end to end.
//!
//! A real till with a real SQLite store, selling with no server reachable, then syncing over real
//! HTTP with real signatures into a real Postgres. Nothing is faked, which is the point: this is
//! the demo that closes deals, and it should fail loudly if it ever stops being true.
//!
//! Skipped unless `SAHL_TEST_DATABASE_URL` is set.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use ed25519_dalek::SigningKey;
use sahl_core::Timestamp;
use sahl_core::money::{Currency, Money, Rounding};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{SaleEvent, TenderMethod};
use sahl_core::tax::{PricingMode, TaxClass};
use sahl_server::db;
use sahl_server::routes::{AppState, router};
use sahl_terminal_lib::store::EventStore;
use sahl_terminal_lib::sync::{HttpTransport, SyncClientError, SyncOutcome, sync_once};
use sahl_terminal_lib::{DeviceIdentity, Terminal};
use sqlx::{Acquire, PgPool};
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("SAHL_TEST_DATABASE_URL").ok()?;
    db::connect(&url, 5).await.ok()
}

struct Shop {
    base: String,
    identity: DeviceIdentity,
    key: SigningKey,
    pool: PgPool,
}

/// Seed a merchant with one enrolled till and start the real server.
async fn open_shop(pool: &PgPool, key_seed: u8) -> Shop {
    let identity = DeviceIdentity {
        tenant_id: Uuid::now_v7(),
        outlet_id: Uuid::now_v7(),
        device_id: Uuid::now_v7(),
    };
    let key = SigningKey::from_bytes(&[key_seed; 32]);

    let mut tx = db::begin_for_tenant(pool, identity.tenant_id)
        .await
        .expect("tx");
    sqlx::query(
        "INSERT INTO tenant (id, name, country_code, default_currency) VALUES ($1,'Karim','BD','BDT')",
    )
    .bind(identity.tenant_id)
    .execute(tx.acquire().await.unwrap())
    .await
    .expect("tenant");
    sqlx::query(
        "INSERT INTO outlet (id, tenant_id, name, profile, timezone, currency) \
         VALUES ($1,$2,'Main','retail','Asia/Dhaka','BDT')",
    )
    .bind(identity.outlet_id)
    .bind(identity.tenant_id)
    .execute(tx.acquire().await.unwrap())
    .await
    .expect("outlet");
    sqlx::query(
        "INSERT INTO device (id, tenant_id, outlet_id, label, public_key) VALUES ($1,$2,$3,'Till 1',$4)",
    )
    .bind(identity.device_id)
    .bind(identity.tenant_id)
    .bind(identity.outlet_id)
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

    Shop {
        base: format!("http://127.0.0.1:{port}"),
        identity,
        key,
        pool: pool.clone(),
    }
}

/// Ring up one complete cash sale on the till.
fn ring_sale(terminal: &mut Terminal, n: u32) -> i64 {
    let sale_id = Uuid::now_v7();
    let clock = 1_753_000_000_000 + i64::from(n) * 1_000;
    // Prices vary so a lost or duplicated sale shifts the total rather than hiding in a round number.
    let total = 4_800 + i64::from(n) * 137;

    let mut at = clock;
    let mut record = |event: &SaleEvent| {
        at += 1;
        terminal
            .record(event, Uuid::now_v7(), Timestamp::from_millis(at))
            .expect("records");
    };

    record(&SaleEvent::Opened {
        sale_id,
        opened_by: Uuid::nil(),
        currency: BDT,
        pricing_mode: PricingMode::TaxInclusive,
        rounding: Rounding::HalfUp,
    });
    record(&SaleEvent::LineAdded {
        sale_id,
        line_id: Uuid::now_v7(),
        product_id: Uuid::nil(),
        name: format!("Item {n}"),
        unit_price: Money::from_minor(total, BDT),
        quantity: Quantity::ONE,
        tax_class: TaxClass::standard(1500),
    });
    record(&SaleEvent::TenderRecorded {
        sale_id,
        tender_id: Uuid::now_v7(),
        method: TenderMethod::Cash,
        amount: Money::from_minor(total, BDT),
        reference: None,
    });
    record(&SaleEvent::Completed {
        sale_id,
        total: Money::from_minor(total, BDT),
        change_given: Money::from_minor(0, BDT),
        at: Timestamp::from_millis(clock + 4),
    });

    total
}

/// Run a sync round on a dedicated OS thread.
///
/// Not a test convenience: `reqwest::blocking` owns an internal runtime that panics if dropped
/// inside an async context, so the blocking transport genuinely has to live off the UI's executor.
/// That is what the production design calls for anyway — the sync loop gets its own thread
/// precisely so it can never block a sale — so the test exercises the same shape.
fn sync_on_thread(
    store: EventStore,
    base: String,
    device: Uuid,
    key: SigningKey,
    rounds: usize,
) -> (EventStore, Result<SyncOutcome, SyncClientError>) {
    std::thread::spawn(move || {
        let mut store = store;
        let mut transport = HttpTransport::new(base, device, key).expect("transport");
        let mut last = Ok(SyncOutcome::default());
        for _ in 0..rounds {
            last = sync_once(&mut store, &mut transport);
            match &last {
                Ok(outcome) if !outcome.more_pending => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        (store, last)
    })
    .join()
    .expect("sync thread")
}

async fn stored_events(pool: &PgPool, identity: DeviceIdentity) -> i64 {
    let mut tx = db::begin_for_tenant(pool, identity.tenant_id)
        .await
        .expect("tx");
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM event WHERE device_id = $1")
        .bind(identity.device_id)
        .fetch_one(tx.acquire().await.unwrap())
        .await
        .expect("count");
    tx.commit().await.expect("commit");
    count.0
}

/// **The unplug test.**
///
/// Fifty sales with the server unreachable, then plug back in. Zero loss, zero duplicates, and the
/// till's takings matching what the server holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_till_sells_through_an_outage_and_reconciles_exactly() {
    let Some(pool) = pool().await else {
        eprintln!("skipped: SAHL_TEST_DATABASE_URL not set");
        return;
    };
    let shop = open_shop(&pool, 21).await;

    let store = EventStore::open_in_memory(shop.identity.device_id).expect("store");
    let mut terminal = Terminal::load(store, shop.identity).expect("loads");

    // --- The internet is down. The shop keeps selling. ---
    let mut expected_takings = 0i64;
    for n in 0..50 {
        expected_takings += ring_sale(&mut terminal, n);
    }

    let (mut store, book) = terminal.into_parts();
    assert_eq!(book.completed().count(), 50, "fifty sales rung offline");
    assert_eq!(
        book.takings(BDT).expect("takings").minor(),
        expected_takings
    );
    assert_eq!(
        store.unsynced_count().expect("count"),
        200,
        "4 events per sale"
    );

    // A failed sync while still offline must change nothing.
    let (store_back, result) = sync_on_thread(
        store,
        "http://127.0.0.1:1".to_owned(),
        shop.identity.device_id,
        shop.key.clone(),
        1,
    );
    store = store_back;
    assert!(result.is_err(), "no server, no sync");
    assert_eq!(
        store.unsynced_count().expect("count"),
        200,
        "a failed sync leaves the queue untouched"
    );

    // --- Plugged back in. ---
    let (store_back, result) = sync_on_thread(
        store,
        shop.base.clone(),
        shop.identity.device_id,
        shop.key.clone(),
        10,
    );
    store = store_back;
    result.expect("syncs");

    assert_eq!(
        store.unsynced_count().expect("count"),
        0,
        "everything pushed"
    );
    assert_eq!(
        stored_events(&shop.pool, shop.identity).await,
        200,
        "zero loss, zero duplicates"
    );

    // --- Syncing again changes nothing. ---
    let (_store, result) = sync_on_thread(
        store,
        shop.base.clone(),
        shop.identity.device_id,
        shop.key.clone(),
        1,
    );
    assert_eq!(result.expect("syncs").pushed, 0);
    assert_eq!(
        stored_events(&shop.pool, shop.identity).await,
        200,
        "a repeat sync must not duplicate"
    );

    // --- The server's copy reconstructs the same takings. ---
    let mut tx = db::begin_for_tenant(&shop.pool, shop.identity.tenant_id)
        .await
        .expect("tx");
    let completed: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM event WHERE device_id = $1 AND kind = 'sale.completed'",
    )
    .bind(shop.identity.device_id)
    .fetch_one(tx.acquire().await.unwrap())
    .await
    .expect("count");
    let server_takings: (Option<i64>,) = sqlx::query_as(
        "SELECT sum((payload->'total'->>'minor')::bigint)::bigint FROM event \
         WHERE device_id = $1 AND kind = 'sale.completed'",
    )
    .bind(shop.identity.device_id)
    .fetch_one(tx.acquire().await.unwrap())
    .await
    .expect("sum");
    tx.commit().await.expect("commit");

    assert_eq!(completed.0, 50);
    assert_eq!(
        server_takings.0,
        Some(expected_takings),
        "the server's totals match the till's, to the paisa"
    );
}

/// A sale rung *after* a partial sync still reaches the server on the next round.
#[tokio::test(flavor = "multi_thread")]
async fn selling_during_a_sync_loses_nothing() {
    let Some(pool) = pool().await else { return };
    let shop = open_shop(&pool, 22).await;

    let store = EventStore::open_in_memory(shop.identity.device_id).expect("store");
    let mut terminal = Terminal::load(store, shop.identity).expect("loads");
    for n in 0..3 {
        ring_sale(&mut terminal, n);
    }
    let (store, _) = terminal.into_parts();

    let (store, result) = sync_on_thread(
        store,
        shop.base.clone(),
        shop.identity.device_id,
        shop.key.clone(),
        3,
    );
    result.expect("first sync");
    assert_eq!(stored_events(&shop.pool, shop.identity).await, 12);

    // The shop keeps trading after the sync.
    let mut terminal = Terminal::load(store, shop.identity).expect("reloads");
    ring_sale(&mut terminal, 99);
    let (store, _) = terminal.into_parts();

    let (_store, result) = sync_on_thread(
        store,
        shop.base.clone(),
        shop.identity.device_id,
        shop.key.clone(),
        3,
    );
    result.expect("second sync");
    assert_eq!(
        stored_events(&shop.pool, shop.identity).await,
        16,
        "the later sale arrived too"
    );
}

/// A revoked till stops syncing, and is told to stop rather than to retry.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_till_is_refused_and_does_not_retry() {
    let Some(pool) = pool().await else { return };
    let shop = open_shop(&pool, 23).await;

    let store = EventStore::open_in_memory(shop.identity.device_id).expect("store");
    let mut terminal = Terminal::load(store, shop.identity).expect("loads");
    ring_sale(&mut terminal, 0);
    let (store, _) = terminal.into_parts();

    let mut tx = db::begin_for_tenant(&pool, shop.identity.tenant_id)
        .await
        .expect("tx");
    sqlx::query("UPDATE device SET revoked_at = now() WHERE id = $1")
        .bind(shop.identity.device_id)
        .execute(tx.acquire().await.unwrap())
        .await
        .expect("revoke");
    tx.commit().await.expect("commit");

    let (_store, result) = sync_on_thread(
        store,
        shop.base.clone(),
        shop.identity.device_id,
        shop.key.clone(),
        1,
    );

    match result {
        Err(SyncClientError::Refused(rejection)) => {
            assert!(
                !rejection.is_retryable(),
                "a stolen till must give up, not hammer"
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(stored_events(&shop.pool, shop.identity).await, 0);
}
