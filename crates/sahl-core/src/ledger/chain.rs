//! The invoice hash chain: ICV plus PIH.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::event::{EventError, EventHash, canonical_bytes};
use crate::money::MoneyError;
use crate::time::Timestamp;

use super::counter::InvoiceCounter;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FiscalError {
    #[error("arithmetic error: {0}")]
    Money(#[from] MoneyError),

    #[error("the invoice could not be canonicalised: {0}")]
    Canonical(#[from] EventError),

    #[error("the invoice counter is exhausted")]
    CounterExhausted,

    #[error("invoice {counter} does not follow {expected}")]
    SequenceBreak { expected: u64, counter: u64 },

    #[error("invoice {counter} claims a predecessor the chain does not have")]
    ChainBroken { counter: u64 },

    #[error("invoice {counter} does not hash to its recorded digest")]
    Tampered { counter: u64 },
}

/// Where a device's fiscal chain has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalTip {
    /// The last invoice counter value issued. Zero means none.
    pub counter: u64,
    /// The last invoice's hash — the next invoice's PIH.
    pub hash: EventHash,
}

impl FiscalTip {
    /// A device that has issued nothing.
    ///
    /// The genesis hash is all zeroes, which is what ZATCA specifies for the first invoice's PIH.
    pub const GENESIS: Self = Self {
        counter: 0,
        hash: EventHash::GENESIS,
    };
}

/// One invoice's place in the fiscal sequence.
///
/// Holds no invoice content, only its digest. The invoice itself lives in the event log, and
/// duplicating it here would create a second copy that can disagree with the first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceSeal {
    pub device_id: Uuid,
    /// The ICV: this device's monotonic invoice counter.
    pub counter: u64,
    /// The sale this invoice settles.
    pub sale_id: Uuid,
    pub issued_at: Timestamp,
    /// The PIH: the previous invoice's hash, or genesis for the first.
    pub previous_hash: EventHash,
    /// This invoice's hash, over the canonical bytes of everything above plus the invoice content.
    pub hash: EventHash,
}

/// A device's fiscal chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiscalChain {
    counter: InvoiceCounter,
    tip: EventHash,
}

impl FiscalChain {
    #[must_use]
    pub const fn new(device_id: Uuid) -> Self {
        Self {
            counter: InvoiceCounter::new(device_id),
            tip: EventHash::GENESIS,
        }
    }

    /// Resume from a stored tip — what a till does at startup.
    #[must_use]
    pub const fn resume(device_id: Uuid, tip: FiscalTip) -> Self {
        Self {
            counter: InvoiceCounter::resume(device_id, tip.counter),
            tip: tip.hash,
        }
    }

    #[must_use]
    pub const fn tip(&self) -> FiscalTip {
        FiscalTip {
            counter: self.counter.issued(),
            hash: self.tip,
        }
    }

    /// Seal an invoice into the chain.
    ///
    /// `content` is whatever the jurisdiction considers the invoice — the totals, the lines, the
    /// registration numbers. It is hashed but not stored, so this stays agnostic about which regime
    /// is in force while still binding the seal to the actual document.
    ///
    /// # Errors
    /// [`FiscalError`] if the counter is exhausted or the content cannot be canonicalised.
    pub fn seal<T: serde::Serialize>(
        &mut self,
        sale_id: Uuid,
        issued_at: Timestamp,
        content: &T,
    ) -> Result<InvoiceSeal, FiscalError> {
        let previous_hash = self.tip;
        let counter = self.counter.next()?;
        let device_id = self.counter.device_id();

        let hash = digest(
            device_id,
            counter,
            sale_id,
            issued_at,
            previous_hash,
            content,
        )?;

        self.tip = hash;
        Ok(InvoiceSeal {
            device_id,
            counter,
            sale_id,
            issued_at,
            previous_hash,
            hash,
        })
    }
}

/// Hash an invoice's place plus its content.
///
/// The predecessor's digest is inside the hashed bytes, not merely stored beside them. That is what
/// makes the chain a chain: changing any earlier invoice changes every later hash, so a removed or
/// edited invoice cannot be hidden by editing one record.
fn digest<T: serde::Serialize>(
    device_id: Uuid,
    counter: u64,
    sale_id: Uuid,
    issued_at: Timestamp,
    previous_hash: EventHash,
    content: &T,
) -> Result<EventHash, FiscalError> {
    #[derive(serde::Serialize)]
    struct Sealed<'a, T> {
        counter: u64,
        device_id: Uuid,
        issued_at: Timestamp,
        previous_hash: String,
        sale_id: Uuid,
        content: &'a T,
    }

    let bytes = canonical_bytes(&Sealed {
        counter,
        device_id,
        issued_at,
        previous_hash: previous_hash.to_hex(),
        sale_id,
        content,
    })?;

    Ok(EventHash::digest(&bytes))
}

/// Verify a device's fiscal chain from a known starting point.
///
/// Proves three things at once, which is why they are checked together rather than in separate
/// passes: no invoice was altered, none was removed, and none was inserted. A gap in the counter
/// and a broken hash link are different symptoms of the same offence.
///
/// # Errors
/// [`FiscalError`] naming the first invoice that does not hold.
pub fn verify_invoice_chain<T: serde::Serialize>(
    seals: &[(InvoiceSeal, T)],
    start: FiscalTip,
) -> Result<FiscalTip, FiscalError> {
    let mut expected_counter = start.counter;
    let mut previous = start.hash;

    for (seal, content) in seals {
        expected_counter = expected_counter
            .checked_add(1)
            .ok_or(FiscalError::CounterExhausted)?;

        if seal.counter != expected_counter {
            return Err(FiscalError::SequenceBreak {
                expected: expected_counter,
                counter: seal.counter,
            });
        }
        if seal.previous_hash != previous {
            return Err(FiscalError::ChainBroken {
                counter: seal.counter,
            });
        }

        let recomputed = digest(
            seal.device_id,
            seal.counter,
            seal.sale_id,
            seal.issued_at,
            seal.previous_hash,
            content,
        )?;
        if recomputed != seal.hash {
            return Err(FiscalError::Tampered {
                counter: seal.counter,
            });
        }

        previous = seal.hash;
    }

    Ok(FiscalTip {
        counter: expected_counter,
        hash: previous,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(n: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + n)
    }

    /// Stands in for whatever a regime considers the invoice.
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
    struct Content {
        total_minor: i64,
    }

    fn content(minor: i64) -> Content {
        Content { total_minor: minor }
    }

    fn three() -> (Vec<(InvoiceSeal, Content)>, FiscalChain) {
        let mut chain = FiscalChain::new(id(3));
        let mut sealed = Vec::new();
        for n in 1..=3_i64 {
            let body = content(n * 10_000);
            let seal = chain
                .seal(id(u128::try_from(n).expect("small")), at(n), &body)
                .expect("seals");
            sealed.push((seal, body));
        }
        (sealed, chain)
    }

    #[test]
    fn the_first_invoice_chains_from_genesis() {
        // ZATCA specifies an all-zero PIH for the first invoice a device issues.
        let mut chain = FiscalChain::new(id(3));
        let seal = chain.seal(id(1), at(0), &content(11_500)).expect("seals");

        assert_eq!(seal.counter, 1);
        assert!(seal.previous_hash.is_genesis());
    }

    #[test]
    fn each_invoice_embeds_its_predecessor() {
        let (sealed, _) = three();

        assert_eq!(sealed[1].0.previous_hash, sealed[0].0.hash);
        assert_eq!(sealed[2].0.previous_hash, sealed[1].0.hash);
    }

    #[test]
    fn a_clean_chain_verifies() {
        let (sealed, chain) = three();
        assert_eq!(
            verify_invoice_chain(&sealed, FiscalTip::GENESIS),
            Ok(chain.tip())
        );
    }

    #[test]
    fn an_altered_invoice_is_caught() {
        // The whole purpose: an amount changed after the fact no longer hashes to its seal.
        let (mut sealed, _) = three();
        sealed[1].1 = content(999);

        assert_eq!(
            verify_invoice_chain(&sealed, FiscalTip::GENESIS),
            Err(FiscalError::Tampered { counter: 2 })
        );
    }

    #[test]
    fn a_removed_invoice_is_caught() {
        // Deleting an inconvenient sale leaves a gap in the counter, which is the point of having
        // one that a database row id could not provide.
        let (mut sealed, _) = three();
        sealed.remove(1);

        assert_eq!(
            verify_invoice_chain(&sealed, FiscalTip::GENESIS),
            Err(FiscalError::SequenceBreak {
                expected: 2,
                counter: 3
            })
        );
    }

    #[test]
    fn a_reordered_chain_is_caught() {
        let (mut sealed, _) = three();
        sealed.swap(0, 1);

        assert!(verify_invoice_chain(&sealed, FiscalTip::GENESIS).is_err());
    }

    #[test]
    fn an_invoice_spliced_in_later_is_caught() {
        // Renumbering it to fit does not help: the PIH it would need belongs to a hash that only
        // exists if every later invoice is reissued too.
        let (mut sealed, _) = three();
        let mut rogue = FiscalChain::new(id(3));
        let body = content(50_000);
        let seal = rogue.seal(id(9), at(9), &body).expect("seals");
        sealed.insert(1, (seal, body));

        assert!(verify_invoice_chain(&sealed, FiscalTip::GENESIS).is_err());
    }

    #[test]
    fn a_restart_continues_the_same_chain() {
        let (sealed, chain) = three();
        let tip = chain.tip();

        let mut resumed = FiscalChain::resume(id(3), tip);
        let body = content(40_000);
        let fourth = resumed.seal(id(4), at(4), &body).expect("seals");

        assert_eq!(fourth.counter, 4);
        assert_eq!(fourth.previous_hash, sealed[2].0.hash);

        let mut all = sealed;
        all.push((fourth, body));
        assert!(verify_invoice_chain(&all, FiscalTip::GENESIS).is_ok());
    }

    #[test]
    fn sealing_is_deterministic() {
        // The terminal and the server both verify this chain and must agree on every digest.
        let mut left = FiscalChain::new(id(3));
        let mut right = FiscalChain::new(id(3));

        assert_eq!(
            left.seal(id(1), at(0), &content(11_500)),
            right.seal(id(1), at(0), &content(11_500))
        );
    }

    #[test]
    fn two_devices_produce_different_digests_for_the_same_sale() {
        // Otherwise two tills ringing identical baskets would collide on their first invoice.
        let mut one = FiscalChain::new(id(1));
        let mut two = FiscalChain::new(id(2));

        assert_ne!(
            one.seal(id(1), at(0), &content(11_500))
                .expect("seals")
                .hash,
            two.seal(id(1), at(0), &content(11_500))
                .expect("seals")
                .hash
        );
    }

    #[test]
    fn verification_can_start_mid_chain() {
        // A server verifying a batch has the tip from the previous batch, not the whole history.
        let (sealed, chain) = three();
        let after_first = FiscalTip {
            counter: 1,
            hash: sealed[0].0.hash,
        };

        assert_eq!(
            verify_invoice_chain(&sealed[1..], after_first),
            Ok(chain.tip())
        );
    }
}
