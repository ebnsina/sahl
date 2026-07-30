//! The reporting endpoints over real HTTP, with real tokens and real row-level security.
//!
//! Skipped unless `SAHL_TEST_DATABASE_URL` is set. The thing worth proving here is not that the
//! arithmetic works — `sahl-core` has property tests for that — but that one shop's token cannot
//! read another shop's day. That is a claim about the database and the wiring, and only a real
//! Postgres running as the real unprivileged role can answer it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_wrap,
    clippy::useless_vec
)]

use sahl_core::Timestamp;
use sahl_core::event::{EventChain, EventEnvelope, EventHeader};
use sahl_core::money::{Currency, Money, Rounding};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{SaleEvent, TenderMethod};
use sahl_core::tax::{PricingMode, TaxClass};
use sahl_server::db;
use sahl_server::device::mint_token;
use sahl_server::routes::{AppState, router};
use sqlx::{Acquire, PgPool};
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("SAHL_TEST_DATABASE_URL").ok()?;
    db::connect(&url, 5).await.ok()
}

struct Shop {
    tenant: Uuid,
    outlet: Uuid,
    device: Uuid,
    /// A token scoped to this outlet alone.
    token: String,
}

/// Create a tenant with one outlet, one device, and a day of trading.
async fn shop(pool: &PgPool, takings: i64) -> Shop {
    let (tenant, outlet, device) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let minted = mint_token().expect("token");

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
        "INSERT INTO device (id, tenant_id, outlet_id, label, public_key) \
         VALUES ($1,$2,$3,'Till',$4)",
    )
    .bind(device)
    .bind(tenant)
    .bind(outlet)
    .bind([7u8; 32].as_slice())
    .execute(tx.acquire().await.unwrap())
    .await
    .expect("device");
    sqlx::query(
        "INSERT INTO dashboard_token (id, tenant_id, outlet_id, label, token_hash) \
         VALUES ($1,$2,$3,'Owner phone',$4)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(outlet)
    .bind(minted.digest.as_slice())
    .execute(tx.acquire().await.unwrap())
    .await
    .expect("token");

    // One settled sale, written the way sync would write it.
    let sale = Uuid::now_v7();
    let events = vec![
        SaleEvent::Opened {
            sale_id: sale,
            opened_by: Uuid::from_u128(0xCA),
            currency: BDT,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        },
        SaleEvent::LineAdded {
            sale_id: sale,
            line_id: Uuid::now_v7(),
            product_id: Uuid::from_u128(7),
            name: "Rice".to_owned(),
            unit_price: Money::from_minor(takings, BDT),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            modifiers: Vec::new(),
        },
        SaleEvent::TenderRecorded {
            sale_id: sale,
            tender_id: Uuid::now_v7(),
            method: TenderMethod::Cash,
            amount: Money::from_minor(takings, BDT),
            reference: None,
        },
        SaleEvent::Completed {
            sale_id: sale,
            total: Money::from_minor(takings, BDT),
            change_given: Money::from_minor(0, BDT),
            at: Timestamp::from_millis(1_753_000_000_000),
        },
    ];

    let mut chain = EventChain::new(device);
    for (index, event) in events.iter().enumerate() {
        let header = EventHeader {
            event_id: Uuid::now_v7(),
            tenant_id: tenant,
            outlet_id: outlet,
            device_id: device,
            occurred_at: Timestamp::from_millis(1_753_000_000_000 + index as i64),
        };
        let envelope: EventEnvelope = chain.append(header, event).expect("seals");
        sqlx::query(
            "INSERT INTO event (event_id, tenant_id, outlet_id, device_id, device_seq, \
             occurred_at, kind, payload, prev_hash, hash) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(envelope.event_id)
        .bind(tenant)
        .bind(outlet)
        .bind(device)
        .bind(i64::try_from(envelope.device_seq).unwrap())
        .bind(envelope.occurred_at.millis())
        .bind(&envelope.kind)
        .bind(&envelope.payload)
        .bind(envelope.prev_hash.as_bytes().as_slice())
        .bind(envelope.hash.as_bytes().as_slice())
        .execute(tx.acquire().await.unwrap())
        .await
        .expect("event");
    }

    tx.commit().await.expect("commit");

    Shop {
        tenant,
        outlet,
        device,
        token: minted.plaintext,
    }
}

async fn serve(pool: &PgPool) -> String {
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
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn a_token_reads_its_own_shops_day() {
    let Some(pool) = pool().await else { return };
    let shop = shop(&pool, 11_500).await;
    let base = serve(&pool).await;

    let response = reqwest::Client::new()
        .get(format!("{base}/api/report/day?outlet={}", shop.outlet))
        .bearer_auth(&shop.token)
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), 200);
    let day: sahl_core::report::Day = response.json().await.expect("json");
    assert_eq!(day.sales, 1);
    assert_eq!(day.takings, Money::from_minor(11_500, BDT));
    assert_eq!(day.tax, Money::from_minor(1_500, BDT), "15% of the base");
    let _ = (shop.tenant, shop.device);
}

#[tokio::test]
async fn one_shops_token_cannot_read_another_shops_day() {
    // The claim the whole tenancy design rests on, tested against the real unprivileged role
    // rather than argued for. Two separate tenants, each with a day on the books.
    let Some(pool) = pool().await else { return };
    let mine = shop(&pool, 11_500).await;
    let theirs = shop(&pool, 99_900).await;
    let base = serve(&pool).await;

    let response = reqwest::Client::new()
        .get(format!("{base}/api/report/day?outlet={}", theirs.outlet))
        .bearer_auth(&mine.token)
        .send()
        .await
        .expect("request");

    assert_eq!(
        response.status(),
        403,
        "a token scoped to one outlet must not read another"
    );
}

#[tokio::test]
async fn a_tenant_wide_token_still_cannot_reach_another_tenant() {
    // The test above proves the handler's own scope check. This one proves the layer underneath
    // it: a token with no outlet restriction asks directly for a stranger's outlet, so nothing in
    // the handler refuses — row-level security has to, and it either does or the whole tenancy
    // story is decoration.
    let Some(pool) = pool().await else { return };
    let mine = shop(&pool, 11_500).await;
    let theirs = shop(&pool, 99_900).await;

    // Widen my token to the whole tenant, exactly as `issue-dashboard-token tenant:…` would.
    let mut tx = db::begin_for_tenant(&pool, mine.tenant)
        .await
        .expect("scoped tx");
    sqlx::query("UPDATE dashboard_token SET outlet_id = NULL WHERE tenant_id = $1")
        .bind(mine.tenant)
        .execute(tx.acquire().await.unwrap())
        .await
        .expect("widen");
    tx.commit().await.expect("commit");

    let base = serve(&pool).await;
    let response = reqwest::Client::new()
        .get(format!("{base}/api/report/day?outlet={}", theirs.outlet))
        .bearer_auth(&mine.token)
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), 200, "not refused — simply empty");
    let day: sahl_core::report::Day = response.json().await.expect("json");
    assert_eq!(day.sales, 0, "another tenant's events are not visible");
    assert_eq!(day.takings.minor(), 0);
}

#[tokio::test]
async fn a_revoked_token_stops_working() {
    let Some(pool) = pool().await else { return };
    let shop = shop(&pool, 11_500).await;

    let mut tx = db::begin_for_tenant(&pool, shop.tenant)
        .await
        .expect("scoped tx");
    sqlx::query("UPDATE dashboard_token SET revoked_at = now() WHERE tenant_id = $1")
        .bind(shop.tenant)
        .execute(tx.acquire().await.unwrap())
        .await
        .expect("revoke");
    tx.commit().await.expect("commit");

    let base = serve(&pool).await;
    let response = reqwest::Client::new()
        .get(format!("{base}/api/report/day?outlet={}", shop.outlet))
        .bearer_auth(&shop.token)
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn an_unknown_token_is_refused_the_same_way_a_revoked_one_is() {
    // Deliberately indistinguishable. Telling the two apart would tell somebody working through
    // guesses when they had guessed a real token.
    let Some(pool) = pool().await else { return };
    let shop = shop(&pool, 11_500).await;
    let base = serve(&pool).await;

    let response = reqwest::Client::new()
        .get(format!("{base}/api/report/day?outlet={}", shop.outlet))
        .bearer_auth("not-a-real-token")
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn no_token_at_all_is_refused() {
    let Some(pool) = pool().await else { return };
    let shop = shop(&pool, 11_500).await;
    let base = serve(&pool).await;

    let response = reqwest::Client::new()
        .get(format!("{base}/api/report/day?outlet={}", shop.outlet))
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn the_outlet_list_shows_only_what_the_token_may_read() {
    let Some(pool) = pool().await else { return };
    let mine = shop(&pool, 11_500).await;
    let _theirs = shop(&pool, 99_900).await;
    let base = serve(&pool).await;

    let response = reqwest::Client::new()
        .get(format!("{base}/api/outlets"))
        .bearer_auth(&mine.token)
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), 200);
    let outlets: Vec<serde_json::Value> = response.json().await.expect("json");
    assert_eq!(outlets.len(), 1);
    assert_eq!(
        outlets[0]["id"].as_str().unwrap(),
        mine.outlet.to_string(),
        "one outlet, and it is theirs"
    );
}
