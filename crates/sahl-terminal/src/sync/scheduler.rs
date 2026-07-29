//! The background sync loop.
//!
//! Runs on its own OS thread and holds the till's lock only for the duration of a round. A cashier
//! must never wait on the network to ring a sale, so this thread is the only place blocking I/O is
//! allowed to happen.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::sync::backoff::Backoff;
use crate::sync::engine::{SyncClientError, Transport};
use crate::terminal::Terminal;

/// What the UI shows about syncing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    /// Everything the till holds is on the server.
    UpToDate { unsynced: u64 },
    /// Trying, and failing. `attempts` drives how loudly the UI complains.
    Retrying { unsynced: u64, attempts: u32 },
    /// Stopped, and will not recover on its own — revoked, forked, or refused.
    ///
    /// Separated from `Retrying` because the two need opposite responses: one is "wait", the other
    /// is "call support". A UI that shows them the same way trains merchants to ignore both.
    Stopped { reason: String },
}

/// Handle on the running loop.
#[derive(Debug, Clone)]
pub struct SyncHandle {
    status: Arc<Mutex<SyncStatus>>,
    running: Arc<AtomicBool>,
}

impl SyncHandle {
    /// Current status, for the UI badge.
    #[must_use]
    pub fn status(&self) -> SyncStatus {
        self.status
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| SyncStatus::Stopped {
                reason: "sync thread panicked".to_owned(),
            })
    }

    /// Ask the loop to finish its current round and stop.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Start syncing in the background.
///
/// `seed` varies the backoff jitter per device — pass something device-specific so a shop's tills
/// do not retry in lockstep after an outage.
pub fn spawn<T>(terminal: Arc<Mutex<Terminal>>, mut transport: T, seed: u64) -> SyncHandle
where
    T: Transport + Send + 'static,
{
    let status = Arc::new(Mutex::new(SyncStatus::UpToDate { unsynced: 0 }));
    let running = Arc::new(AtomicBool::new(true));

    let handle = SyncHandle {
        status: Arc::clone(&status),
        running: Arc::clone(&running),
    };

    std::thread::Builder::new()
        .name("sahl-sync".to_owned())
        .spawn(move || {
            let mut backoff = Backoff::standard(seed);

            while running.load(Ordering::Relaxed) {
                // The lock is taken for the round and released before sleeping. Holding it across
                // the sleep would block every sale for the length of the backoff.
                let outcome = {
                    let Ok(mut till) = terminal.lock() else {
                        set(
                            &status,
                            SyncStatus::Stopped {
                                reason: "the till is in an inconsistent state".to_owned(),
                            },
                        );
                        return;
                    };
                    till.sync(&mut transport)
                };

                let pending = terminal
                    .lock()
                    .ok()
                    .and_then(|till| till.unsynced_count().ok())
                    .unwrap_or(0);

                match outcome {
                    Ok(result) => {
                        backoff.reset();
                        set(&status, SyncStatus::UpToDate { unsynced: pending });

                        // More waiting means go again now rather than idling for thirty seconds
                        // with a merchant's sales still only on this device.
                        if result.more_pending {
                            continue;
                        }
                        sleep_interruptibly(Backoff::IDLE, &running);
                    }
                    Err(error) => {
                        if is_terminal_failure(&error) {
                            set(
                                &status,
                                SyncStatus::Stopped {
                                    reason: error.to_string(),
                                },
                            );
                            return;
                        }
                        let delay = backoff.next_delay();
                        set(
                            &status,
                            SyncStatus::Retrying {
                                unsynced: pending,
                                attempts: backoff.attempts(),
                            },
                        );
                        sleep_interruptibly(delay, &running);
                    }
                }
            }
        })
        .ok();

    handle
}

/// Whether retrying could ever help.
///
/// A fork or a revocation needs a person. Retrying those forever would bury the real signal in a
/// log full of identical failures, and leave the merchant believing sync is merely slow.
fn is_terminal_failure(error: &SyncClientError) -> bool {
    match error {
        SyncClientError::Refused(rejection) => !rejection.is_retryable(),
        SyncClientError::TipMismatch { .. } => true,
        SyncClientError::Store(_) => false,
    }
}

/// Sleep in short slices so a shutdown does not wait out a five-minute backoff.
fn sleep_interruptibly(total: Duration, running: &AtomicBool) {
    const SLICE: Duration = Duration::from_millis(250);
    let mut slept = Duration::ZERO;
    while slept < total && running.load(Ordering::Relaxed) {
        let step = SLICE.min(total.saturating_sub(slept));
        std::thread::sleep(step);
        slept = slept.saturating_add(step);
    }
}

fn set(status: &Arc<Mutex<SyncStatus>>, next: SyncStatus) {
    if let Ok(mut guard) = status.lock() {
        *guard = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::EventStore;
    use crate::terminal::DeviceIdentity;
    use sahl_core::event::EventEnvelope;
    use sahl_sync::{PullResponse, PushResponse, SyncRejection};
    use uuid::Uuid;

    fn identity() -> DeviceIdentity {
        DeviceIdentity {
            tenant_id: Uuid::from_u128(1),
            outlet_id: Uuid::from_u128(2),
            device_id: Uuid::from_u128(3),
        }
    }

    fn till() -> Arc<Mutex<Terminal>> {
        let store = EventStore::open_in_memory(identity().device_id).expect("store");
        Arc::new(Mutex::new(
            Terminal::load(store, identity()).expect("loads"),
        ))
    }

    /// Always fails with the given rejection, counting attempts.
    struct AlwaysFails {
        rejection: SyncRejection,
        calls: Arc<AtomicBool>,
    }

    impl Transport for AlwaysFails {
        fn push(&mut self, _: &[EventEnvelope]) -> Result<PushResponse, SyncRejection> {
            self.calls.store(true, Ordering::Relaxed);
            Err(self.rejection)
        }
        fn pull(&mut self, _: i64, _: usize) -> Result<PullResponse, SyncRejection> {
            self.calls.store(true, Ordering::Relaxed);
            Err(self.rejection)
        }
    }

    #[test]
    fn a_revocation_stops_the_loop_rather_than_retrying_forever() {
        // Retrying a revoked device forever buries the real signal and tells the merchant sync is
        // merely slow.
        let called = Arc::new(AtomicBool::new(false));
        let handle = spawn(
            till(),
            AlwaysFails {
                rejection: SyncRejection::NotAuthorised,
                calls: Arc::clone(&called),
            },
            7,
        );

        for _ in 0..40 {
            if matches!(handle.status(), SyncStatus::Stopped { .. }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        assert!(called.load(Ordering::Relaxed), "it did try once");
        assert!(matches!(handle.status(), SyncStatus::Stopped { .. }));
    }

    #[test]
    fn an_unreachable_server_keeps_retrying() {
        let handle = spawn(
            till(),
            AlwaysFails {
                rejection: SyncRejection::Unavailable,
                calls: Arc::new(AtomicBool::new(false)),
            },
            11,
        );

        let mut saw_retrying = false;
        for _ in 0..40 {
            if matches!(handle.status(), SyncStatus::Retrying { .. }) {
                saw_retrying = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        handle.stop();

        assert!(saw_retrying, "a flaky link is not a permanent failure");
    }

    #[test]
    fn stopping_interrupts_a_long_backoff() {
        // Slicing the sleep is why shutdown does not wait out five minutes.
        let handle = spawn(
            till(),
            AlwaysFails {
                rejection: SyncRejection::Unavailable,
                calls: Arc::new(AtomicBool::new(false)),
            },
            13,
        );
        std::thread::sleep(Duration::from_millis(100));

        let before = std::time::Instant::now();
        handle.stop();
        std::thread::sleep(Duration::from_millis(400));

        assert!(
            before.elapsed() < Duration::from_secs(2),
            "shutdown should not wait out the backoff"
        );
    }
}
