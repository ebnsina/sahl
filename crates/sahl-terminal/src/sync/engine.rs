use sahl_core::event::EventEnvelope;
use sahl_sync::{MAX_BATCH, PullResponse, PushResponse, SyncRejection};
use thiserror::Error;

use crate::store::{EventStore, StoreError};

/// How the till reaches the server.
///
/// A trait so the engine's logic — batching, acking, cursor advance, backoff decisions — is
/// testable without a network. Every interesting failure here is a *network* failure, and the ones
/// worth testing (lost acks, partial progress, refusals) are precisely the ones hardest to provoke
/// against a real server.
pub trait Transport {
    /// Offer events. `Err` means the server refused; the reason decides whether to retry.
    fn push(&mut self, events: &[EventEnvelope]) -> Result<PushResponse, SyncRejection>;

    /// Ask for events from sibling devices above `cursor`.
    fn pull(&mut self, cursor: i64, limit: usize) -> Result<PullResponse, SyncRejection>;
}

#[derive(Debug, Error)]
pub enum SyncClientError {
    #[error("storage error: {0}")]
    Store(#[from] StoreError),

    #[error("server refused the batch: {0:?}")]
    Refused(SyncRejection),

    /// The server's chain tip disagrees with ours after a successful push.
    ///
    /// No hash check catches this: a truncated local log is internally consistent, it is just
    /// *short*. Comparing tips is the only way to notice, and noticing matters because the device
    /// would otherwise keep appending onto a history the server never agreed to.
    #[error("local log disagrees with the server: ours ends at {ours}, server says {theirs}")]
    TipMismatch { ours: u64, theirs: u64 },
}

/// What one sync round did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncOutcome {
    pub pushed: usize,
    pub skipped: usize,
    pub pulled: usize,
    /// More is waiting — the caller should run again immediately rather than wait for the timer.
    pub more_pending: bool,
}

/// Push everything outstanding, then pull everything new.
///
/// Push first, deliberately. A shop's own sales are the data that only exists on this device; a
/// sibling's sales are already safe on the server. If a round is cut short by a flaky connection,
/// the half that ran should be the half that reduces risk.
///
/// # Errors
/// [`SyncClientError`] on refusal, storage failure, or tip disagreement.
pub fn sync_once(
    store: &mut EventStore,
    transport: &mut impl Transport,
) -> Result<SyncOutcome, SyncClientError> {
    let mut outcome = push_pending(store, transport)?;
    let pulled = pull_new(store, transport)?;

    outcome.pulled = pulled.0;
    outcome.more_pending = outcome.more_pending || pulled.1;
    Ok(outcome)
}

fn push_pending(
    store: &mut EventStore,
    transport: &mut impl Transport,
) -> Result<SyncOutcome, SyncClientError> {
    let pending = store.unsynced()?;
    if pending.is_empty() {
        return Ok(SyncOutcome::default());
    }

    // One bounded batch per round. A till offline for a week uploads over several rounds rather
    // than one request that times out and retries forever, making no progress.
    let batch = pending.get(..MAX_BATCH).unwrap_or(&pending);
    let response = transport.push(batch).map_err(SyncClientError::Refused)?;

    let ours = store.tip()?;
    let sent_through = batch
        .last()
        .map_or(ours.device_seq, |event| event.device_seq);

    // The server's tip must cover what we just sent. If it is behind, our log has events the server
    // never received — which after a *successful* push means our local log was truncated.
    if response.tip.device_seq < sent_through {
        return Err(SyncClientError::TipMismatch {
            ours: sent_through,
            theirs: response.tip.device_seq,
        });
    }

    // Only ack what the server confirmed. Marking optimistically would drop events on the floor if
    // the transaction rolled back after replying.
    store.mark_synced(response.tip.device_seq, now_millis())?;

    Ok(SyncOutcome {
        pushed: response.accepted,
        skipped: response.skipped,
        pulled: 0,
        more_pending: pending.len() > batch.len(),
    })
}

fn pull_new(
    store: &mut EventStore,
    transport: &mut impl Transport,
) -> Result<(usize, bool), SyncClientError> {
    let cursor = store.pull_cursor()?;
    let page = transport
        .pull(cursor, PULL_PAGE)
        .map_err(SyncClientError::Refused)?;

    let mut applied = 0usize;
    for event in &page.events {
        // Verify before storing. A sibling's events arrive over the network, and the whole point of
        // sealing them is that this device can check them rather than trust the transport.
        event
            .verify()
            .map_err(|_| SyncClientError::Refused(SyncRejection::Invalid))?;
        if store.insert_remote(event, page.cursor)? {
            applied = applied.saturating_add(1);
        }
    }

    // Advance even when the page was empty or entirely duplicates — otherwise a page of events this
    // device already holds would be requested forever.
    store.set_pull_cursor(page.cursor)?;
    Ok((applied, page.has_more))
}

/// Events requested per pull.
const PULL_PAGE: usize = 200;

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
    use sahl_core::Timestamp;
    use sahl_core::event::{ChainTip, EventChain, EventHeader, EventPayload};
    use serde::Serialize;
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

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    const OURS: u128 = 0x0A;
    const SIBLING: u128 = 0x0B;

    fn seal(chain: &mut EventChain, device: u128, n: u32) -> EventEnvelope {
        chain
            .append(
                EventHeader {
                    event_id: id(0x1000 + u128::from(n) + device * 0x100),
                    tenant_id: id(1),
                    outlet_id: id(2),
                    device_id: id(device),
                    occurred_at: Timestamp::from_millis(1_753_000_000_000 + i64::from(n)),
                },
                &Sale { n },
            )
            .expect("seals")
    }

    /// A server that accepts everything, recording what it saw.
    #[derive(Default)]
    struct FakeServer {
        held: Vec<EventEnvelope>,
        to_deliver: Vec<EventEnvelope>,
        push_calls: usize,
        /// Set to make the next push report a tip behind what was sent.
        understate_tip: bool,
        refuse_with: Option<SyncRejection>,
    }

    impl Transport for FakeServer {
        fn push(&mut self, events: &[EventEnvelope]) -> Result<PushResponse, SyncRejection> {
            self.push_calls = self.push_calls.saturating_add(1);
            if let Some(rejection) = self.refuse_with {
                return Err(rejection);
            }

            let mut accepted = 0;
            for event in events {
                if !self.held.iter().any(|held| held.event_id == event.event_id) {
                    self.held.push(event.clone());
                    accepted += 1;
                }
            }

            let top = self
                .held
                .last()
                .map_or(ChainTip::GENESIS, |event| ChainTip {
                    device_seq: if self.understate_tip {
                        event.device_seq.saturating_sub(1)
                    } else {
                        event.device_seq
                    },
                    hash: event.hash,
                });

            Ok(PushResponse {
                accepted,
                skipped: events.len().saturating_sub(accepted),
                tip: top,
                high_water: i64::try_from(self.held.len()).unwrap_or(0),
            })
        }

        fn pull(&mut self, cursor: i64, limit: usize) -> Result<PullResponse, SyncRejection> {
            if let Some(rejection) = self.refuse_with {
                return Err(rejection);
            }
            let start = usize::try_from(cursor).unwrap_or(0);
            let slice = self.to_deliver.get(start..).unwrap_or(&[]);
            let page = slice.get(..limit).unwrap_or(slice);

            Ok(PullResponse {
                events: page.to_vec(),
                cursor: cursor.saturating_add(i64::try_from(page.len()).unwrap_or(0)),
                has_more: slice.len() > page.len(),
            })
        }
    }

    fn till_with(count: u32) -> (EventStore, EventChain) {
        let mut store = EventStore::open_in_memory(id(OURS)).expect("opens");
        let mut chain = EventChain::new(id(OURS));
        for n in 0..count {
            let event = seal(&mut chain, OURS, n);
            store.append(&event).expect("stores");
        }
        (store, chain)
    }

    #[test]
    fn a_quiet_till_syncs_without_doing_anything() {
        let (mut store, _) = till_with(0);
        let mut server = FakeServer::default();

        let outcome = sync_once(&mut store, &mut server).expect("syncs");
        assert_eq!(outcome, SyncOutcome::default());
        assert_eq!(server.push_calls, 0, "no events, no request");
    }

    #[test]
    fn pending_sales_are_pushed_and_acked() {
        let (mut store, _) = till_with(12);
        let mut server = FakeServer::default();

        let outcome = sync_once(&mut store, &mut server).expect("syncs");

        assert_eq!(outcome.pushed, 12);
        assert_eq!(store.unsynced_count().expect("counts"), 0);
        assert_eq!(server.held.len(), 12);
    }

    #[test]
    fn a_second_round_sends_nothing_new() {
        let (mut store, _) = till_with(5);
        let mut server = FakeServer::default();

        sync_once(&mut store, &mut server).expect("first");
        let second = sync_once(&mut store, &mut server).expect("second");

        assert_eq!(second.pushed, 0);
        assert_eq!(server.held.len(), 5, "no duplicates");
    }

    #[test]
    fn events_are_only_acked_once_the_server_confirms_them() {
        // Marking optimistically would drop events if the server's transaction rolled back.
        let (mut store, _) = till_with(4);
        let mut server = FakeServer {
            refuse_with: Some(SyncRejection::Unavailable),
            ..FakeServer::default()
        };

        let result = sync_once(&mut store, &mut server);

        assert!(result.is_err());
        assert_eq!(
            store.unsynced_count().expect("counts"),
            4,
            "a refused push leaves everything queued"
        );
    }

    #[test]
    fn a_server_tip_behind_our_own_is_a_hard_error() {
        // A truncated local log is internally consistent — just short. Comparing tips is the only
        // way to catch it, and continuing would append onto a history the server never agreed to.
        let (mut store, _) = till_with(6);
        let mut server = FakeServer {
            understate_tip: true,
            ..FakeServer::default()
        };

        assert!(matches!(
            sync_once(&mut store, &mut server),
            Err(SyncClientError::TipMismatch { .. })
        ));
    }

    #[test]
    fn a_siblings_sales_are_pulled_and_stored() {
        let (mut store, _) = till_with(2);
        let mut sibling_chain = EventChain::new(id(SIBLING));
        let mut server = FakeServer {
            to_deliver: (0..4)
                .map(|n| seal(&mut sibling_chain, SIBLING, n))
                .collect(),
            ..FakeServer::default()
        };

        let outcome = sync_once(&mut store, &mut server).expect("syncs");

        assert_eq!(outcome.pulled, 4);
        assert_eq!(store.pull_cursor().expect("cursor"), 4);
        // Our own chain is untouched by the sibling's sequence numbers.
        assert_eq!(store.tip().expect("tip").device_seq, 2);
    }

    #[test]
    fn a_redelivered_page_is_absorbed_without_duplicating() {
        // Pull pages overlap after a crash; a till that chokes on that stops syncing.
        let (mut store, _) = till_with(1);
        let mut sibling_chain = EventChain::new(id(SIBLING));
        let delivered: Vec<_> = (0..3)
            .map(|n| seal(&mut sibling_chain, SIBLING, n))
            .collect();
        let mut server = FakeServer {
            to_deliver: delivered,
            ..FakeServer::default()
        };

        sync_once(&mut store, &mut server).expect("first");
        store.set_pull_cursor(0).expect("rewind attempt");
        let second = sync_once(&mut store, &mut server).expect("second");

        assert_eq!(second.pulled, 0, "already held");
    }

    #[test]
    fn the_cursor_never_moves_backwards() {
        // A stale response arriving late must not rewind progress and replay a page forever.
        let (mut store, _) = till_with(0);
        store.set_pull_cursor(50).expect("advance");
        store.set_pull_cursor(10).expect("stale response");

        assert_eq!(store.pull_cursor().expect("cursor"), 50);
    }

    #[test]
    fn a_tampered_remote_event_is_refused_rather_than_stored() {
        // A sibling's events arrive over the network. Sealing them is only worth anything if this
        // device actually checks them.
        let (mut store, _) = till_with(0);
        let mut sibling_chain = EventChain::new(id(SIBLING));
        let mut forged = seal(&mut sibling_chain, SIBLING, 0);
        forged.payload = serde_json::json!({ "n": 999 });

        let mut server = FakeServer {
            to_deliver: vec![forged],
            ..FakeServer::default()
        };

        assert!(matches!(
            sync_once(&mut store, &mut server),
            Err(SyncClientError::Refused(SyncRejection::Invalid))
        ));
        assert_eq!(
            store.pull_cursor().expect("cursor"),
            0,
            "cursor did not advance"
        );
    }

    #[test]
    fn a_large_backlog_reports_more_pending() {
        let (mut store, _) = till_with(0);
        let mut chain = EventChain::new(id(OURS));
        for n in 0..(u32::try_from(MAX_BATCH).unwrap_or(500) + 10) {
            let event = seal(&mut chain, OURS, n);
            store.append(&event).expect("stores");
        }

        let mut server = FakeServer::default();
        let outcome = sync_once(&mut store, &mut server).expect("syncs");

        assert_eq!(outcome.pushed, MAX_BATCH, "one bounded batch per round");
        assert!(outcome.more_pending, "caller should run again immediately");
        assert_eq!(store.unsynced_count().expect("counts"), 10);
    }
}
