//! Batches, and why a POS in this market has to know about them.
//!
//! A pharmacy cannot answer "which customers received lot X" without recording which batch each
//! sale drew from, and a recall makes that question urgent rather than theoretical. A grocery
//! cannot sell down its stock sensibly without knowing what expires first.
//!
//! Batches are therefore identity, not metadata: two crates of the same product with different
//! expiry dates are different things, and the software should never quietly merge them.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::time::Timestamp;

/// One received lot of a product.
///
/// Equality is by `id`, not by contents. Two deliveries with the same printed lot number from
/// different suppliers are still separate arrivals, and conflating them would make a recall
/// under-report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub id: Uuid,
    pub product_id: Uuid,
    /// The lot number printed on the packaging, as received. Not unique on its own.
    pub lot: Option<String>,
    /// When it stops being sellable. `None` for goods that do not expire.
    pub expires_at: Option<Timestamp>,
    /// When it arrived — the tiebreak when two batches share an expiry.
    pub received_at: Timestamp,
}

impl Batch {
    #[must_use]
    pub fn is_expired(&self, now: Timestamp) -> bool {
        self.expires_at
            .is_some_and(|expiry| now.millis() >= expiry.millis())
    }

    /// Whether this batch expires within `window_millis`.
    ///
    /// What drives the "use or discount this soon" list. Already-expired stock is excluded — that
    /// is a different problem with a different answer, and mixing them makes the list unusable.
    #[must_use]
    pub fn expires_within(&self, now: Timestamp, window_millis: i64) -> bool {
        let Some(expiry) = self.expires_at else {
            return false;
        };
        let remaining = expiry.millis().saturating_sub(now.millis());
        remaining > 0 && remaining <= window_millis
    }

    /// Sort key for first-expired-first-out.
    ///
    /// Non-expiring stock sorts last, so a batch with a date always leaves before one without —
    /// the whole point of FEFO. Ties break on arrival, then on id, so allocation is deterministic
    /// across devices.
    #[must_use]
    pub fn fefo_key(&self) -> (i64, i64, Uuid) {
        (
            self.expires_at.map_or(i64::MAX, Timestamp::millis),
            self.received_at.millis(),
            self.id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn day(n: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + n * 86_400_000)
    }

    fn batch(n: u128, expiry: Option<i64>, received: i64) -> Batch {
        Batch {
            id: id(n),
            product_id: id(0x99),
            lot: Some(format!("LOT{n}")),
            expires_at: expiry.map(day),
            received_at: day(received),
        }
    }

    #[test]
    fn expiry_is_inclusive_of_the_moment_it_arrives() {
        let stock = batch(1, Some(10), 0);
        assert!(!stock.is_expired(day(9)));
        assert!(stock.is_expired(day(10)));
    }

    #[test]
    fn goods_without_a_date_never_expire() {
        let rice = batch(1, None, 0);
        assert!(!rice.is_expired(day(10_000)));
        assert!(!rice.expires_within(day(0), 86_400_000 * 30));
    }

    #[test]
    fn the_expiring_soon_list_excludes_stock_that_already_went() {
        // Expired and expiring are different problems with different answers; mixing them makes
        // the list something a shopkeeper stops reading.
        let soon = batch(1, Some(5), 0);
        let gone = batch(2, Some(-1), -10);
        let window = 86_400_000 * 7;

        assert!(soon.expires_within(day(0), window));
        assert!(!gone.expires_within(day(0), window), "already expired");
        assert!(gone.is_expired(day(0)));
    }

    #[test]
    fn fefo_sells_the_soonest_expiring_first() {
        let mut batches = [
            batch(1, Some(30), 0),
            batch(2, Some(5), 1),
            batch(3, Some(15), 2),
        ];
        batches.sort_by_key(Batch::fefo_key);

        assert_eq!(
            batches.iter().map(|b| b.id).collect::<Vec<_>>(),
            vec![id(2), id(3), id(1)]
        );
    }

    #[test]
    fn dated_stock_always_leaves_before_undated() {
        let mut batches = [batch(1, None, 0), batch(2, Some(9_000), 5)];
        batches.sort_by_key(Batch::fefo_key);

        assert_eq!(
            batches[0].id,
            id(2),
            "even a distant expiry beats no expiry"
        );
    }

    #[test]
    fn batches_sharing_an_expiry_leave_in_arrival_order() {
        let mut batches = [batch(1, Some(10), 5), batch(2, Some(10), 1)];
        batches.sort_by_key(Batch::fefo_key);

        assert_eq!(batches[0].id, id(2), "older delivery first");
    }

    #[test]
    fn the_same_lot_from_two_deliveries_stays_two_batches() {
        // Conflating them would make a recall under-report.
        let mut first = batch(1, Some(10), 0);
        let mut second = batch(2, Some(10), 0);
        first.lot = Some("AB123".to_owned());
        second.lot = Some("AB123".to_owned());

        assert_ne!(first, second);
    }
}
