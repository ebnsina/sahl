use sahl_core::event::{ChainTip, EventEnvelope, EventHash};
use sahl_sync::{PullResponse, PushResponse, SyncError, plan_batch};
use sqlx::{Acquire, Postgres, Transaction};
use uuid::Uuid;

use super::error::IngestError;

/// The device row as sqlx returns it.
type DeviceRow = (
    Uuid,
    Uuid,
    Vec<u8>,
    Option<time::OffsetDateTime>,
    i64,
    Vec<u8>,
);

/// An event row as sqlx returns it, in `SELECT` order.
type EventRow = (
    Uuid,
    Uuid,
    Uuid,
    Uuid,
    i64,
    i64,
    String,
    serde_json::Value,
    Vec<u8>,
    Vec<u8>,
    i64,
);

/// A device as the server knows it, loaded inside the request's transaction.
#[derive(Debug, Clone, Copy)]
pub struct DeviceRecord {
    pub device_id: Uuid,
    pub tenant_id: Uuid,
    pub outlet_id: Uuid,
    pub public_key: [u8; 32],
    pub revoked: bool,
    pub tip: ChainTip,
}

/// Load a device and its chain tip.
///
/// # Errors
/// [`IngestError::UnknownDevice`] if absent, [`IngestError::Database`] on failure.
pub async fn load_device(
    transaction: &mut Transaction<'static, Postgres>,
    device_id: Uuid,
) -> Result<DeviceRecord, IngestError> {
    let row: Option<DeviceRow> = sqlx::query_as(
        "SELECT tenant_id, outlet_id, public_key, revoked_at, last_device_seq, last_hash \
             FROM device WHERE id = $1",
    )
    .bind(device_id)
    .fetch_optional(transaction.acquire().await?)
    .await?;

    let Some((tenant_id, outlet_id, key, revoked_at, last_seq, last_hash)) = row else {
        return Err(IngestError::UnknownDevice { device_id });
    };

    let public_key: [u8; 32] = key
        .try_into()
        .map_err(|_| IngestError::CorruptDevice { device_id })?;
    let hash: [u8; 32] = last_hash
        .try_into()
        .map_err(|_| IngestError::CorruptDevice { device_id })?;

    Ok(DeviceRecord {
        device_id,
        tenant_id,
        outlet_id,
        public_key,
        revoked: revoked_at.is_some(),
        tip: ChainTip {
            device_seq: u64::try_from(last_seq)
                .map_err(|_| IngestError::CorruptDevice { device_id })?,
            hash: EventHash::from_bytes(hash),
        },
    })
}

/// Accept a batch of events.
///
/// Runs entirely inside the caller's transaction, so a batch is all-or-nothing. A half-committed
/// batch would leave a chain the terminal cannot resume from — worse than rejecting it outright.
///
/// # Errors
/// [`IngestError`] if the device is revoked, the batch is inconsistent, or the database fails.
pub async fn push(
    transaction: &mut Transaction<'static, Postgres>,
    device: &DeviceRecord,
    events: &[EventEnvelope],
) -> Result<PushResponse, IngestError> {
    if device.revoked {
        return Err(IngestError::Revoked {
            device_id: device.device_id,
        });
    }

    let plan = plan_batch(events, device.tip, device.device_id).map_err(IngestError::Rejected)?;

    if plan.is_noop() {
        // A retry whose original response was lost. Report the stored tip so the terminal can
        // reconcile and mark its events synced.
        let high_water = high_water_for(transaction, device.device_id).await?;
        return Ok(PushResponse {
            accepted: 0,
            skipped: plan.already_stored,
            tip: device.tip,
            high_water,
        });
    }

    let fresh = events
        .get(plan.already_stored..)
        .ok_or(IngestError::Rejected(SyncError::Gap {
            tip: device.tip.device_seq,
            batch_starts_at: 0,
        }))?;

    let mut high_water = 0i64;
    for event in fresh {
        // Every event carries the tenant and outlet it belongs to, but the *device* record is the
        // authority. A terminal claiming another outlet's id would otherwise write across the
        // tenancy boundary, which RLS would catch — but catching it here names the problem.
        if event.tenant_id != device.tenant_id || event.outlet_id != device.outlet_id {
            return Err(IngestError::ScopeMismatch {
                device_id: device.device_id,
                event_id: event.event_id,
            });
        }

        let seq = i64::try_from(event.device_seq).map_err(|_| IngestError::CorruptDevice {
            device_id: device.device_id,
        })?;

        let assigned: (i64,) = sqlx::query_as(
            "INSERT INTO event (
                 event_id, tenant_id, outlet_id, device_id, device_seq,
                 occurred_at, kind, payload, prev_hash, hash
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             RETURNING server_seq",
        )
        .bind(event.event_id)
        .bind(event.tenant_id)
        .bind(event.outlet_id)
        .bind(event.device_id)
        .bind(seq)
        .bind(event.occurred_at.millis())
        .bind(&event.kind)
        .bind(&event.payload)
        .bind(event.prev_hash.as_bytes().as_slice())
        .bind(event.hash.as_bytes().as_slice())
        .fetch_one(transaction.acquire().await?)
        .await?;

        high_water = high_water.max(assigned.0);
    }

    let new_seq =
        i64::try_from(plan.resulting_tip.device_seq).map_err(|_| IngestError::CorruptDevice {
            device_id: device.device_id,
        })?;

    sqlx::query(
        "UPDATE device SET last_device_seq = $1, last_hash = $2, last_seen_at = now() WHERE id = $3",
    )
    .bind(new_seq)
    .bind(plan.resulting_tip.hash.as_bytes().as_slice())
    .bind(device.device_id)
    .execute(transaction.acquire().await?)
    .await?;

    Ok(PushResponse {
        accepted: plan.to_insert,
        skipped: plan.already_stored,
        tip: plan.resulting_tip,
        high_water,
    })
}

/// Events from the outlet's *other* devices, above `cursor`.
///
/// A device's own events are excluded: it already has them, and echoing them back would double a
/// busy shop's sync traffic for no gain.
///
/// # Errors
/// [`IngestError::Database`] on failure.
pub async fn pull(
    transaction: &mut Transaction<'static, Postgres>,
    device: &DeviceRecord,
    cursor: i64,
    limit: usize,
) -> Result<PullResponse, IngestError> {
    // Fetch one extra to answer `has_more` without a second count query.
    let fetch = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);

    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT event_id, tenant_id, outlet_id, device_id, device_seq, occurred_at, \
                kind, payload, prev_hash, hash, server_seq \
         FROM event \
         WHERE outlet_id = $1 AND device_id <> $2 AND server_seq > $3 \
         ORDER BY server_seq LIMIT $4",
    )
    .bind(device.outlet_id)
    .bind(device.device_id)
    .bind(cursor)
    .bind(fetch)
    .fetch_all(transaction.acquire().await?)
    .await?;

    let has_more = rows.len() > limit;
    let page = rows.get(..limit).unwrap_or(&rows);

    let mut events = Vec::with_capacity(page.len());
    let mut next = cursor;
    for row in page {
        let (
            event_id,
            tenant_id,
            outlet_id,
            device_id,
            device_seq,
            occurred_at,
            kind,
            payload,
            prev_hash,
            hash,
            server_seq,
        ) = row;

        events.push(EventEnvelope {
            event_id: *event_id,
            tenant_id: *tenant_id,
            outlet_id: *outlet_id,
            device_id: *device_id,
            device_seq: u64::try_from(*device_seq).unwrap_or(0),
            occurred_at: sahl_core::Timestamp::from_millis(*occurred_at),
            kind: kind.clone(),
            payload: payload.clone(),
            prev_hash: decode(prev_hash)?,
            hash: decode(hash)?,
        });
        next = *server_seq;
    }

    Ok(PullResponse {
        events,
        cursor: next,
        has_more,
    })
}

async fn high_water_for(
    transaction: &mut Transaction<'static, Postgres>,
    device_id: Uuid,
) -> Result<i64, IngestError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT max(server_seq) FROM event WHERE device_id = $1")
            .bind(device_id)
            .fetch_optional(transaction.acquire().await?)
            .await?;
    Ok(row.map_or(0, |(value,)| value))
}

fn decode(bytes: &[u8]) -> Result<EventHash, IngestError> {
    let sized: [u8; 32] = bytes.try_into().map_err(|_| IngestError::CorruptEvent)?;
    Ok(EventHash::from_bytes(sized))
}
