//! The invoice counter (ICV).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::chain::FiscalError;

/// A per-device monotonic invoice counter.
///
/// Per *device*, not per outlet, and that is not a simplification. Two tills in one shop are offline
/// from each other by design, so a shared counter would need coordination the product deliberately
/// does not have — and the first network outage would produce two invoices numbered the same. ZATCA
/// scopes the ICV to the device for exactly this reason.
///
/// Starts at zero and issues from one. There is no invoice zero, and a regime reading a sequence
/// that starts at zero reads a device that has issued something it cannot show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceCounter {
    device_id: Uuid,
    issued: u64,
}

impl InvoiceCounter {
    /// A device that has never issued an invoice.
    #[must_use]
    pub const fn new(device_id: Uuid) -> Self {
        Self {
            device_id,
            issued: 0,
        }
    }

    /// Resume from a stored count — what a till does when it restarts mid-day.
    #[must_use]
    pub const fn resume(device_id: Uuid, issued: u64) -> Self {
        Self { device_id, issued }
    }

    /// Take the next number.
    ///
    /// Named `next` despite the iterator resemblance because this *is* the next invoice number and
    /// any other name reads worse at the call site. It is fallible and mutating, so nothing here
    /// could be mistaken for an iterator in practice.
    ///
    /// # Errors
    /// [`FiscalError::CounterExhausted`] on overflow. Unreachable in practice — a till issuing an
    /// invoice a second would take longer than the universe has existed — but a silent wrap here
    /// would restart the sequence at zero, so it is refused rather than saturated.
    #[expect(
        clippy::should_implement_trait,
        reason = "an invoice counter is not an iterator: it is fallible, unbounded, and its values \
                  are issued rather than traversed"
    )]
    pub fn next(&mut self) -> Result<u64, FiscalError> {
        let next = self
            .issued
            .checked_add(1)
            .ok_or(FiscalError::CounterExhausted)?;
        self.issued = next;
        Ok(next)
    }

    /// How many invoices this device has issued.
    #[must_use]
    pub const fn issued(self) -> u64 {
        self.issued
    }

    #[must_use]
    pub const fn device_id(self) -> Uuid {
        self.device_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn the_first_invoice_is_one_not_zero() {
        // A sequence starting at zero reads as a device that issued something it cannot show.
        let mut counter = InvoiceCounter::new(id(1));
        assert_eq!(counter.next(), Ok(1));
        assert_eq!(counter.next(), Ok(2));
    }

    #[test]
    fn a_restart_continues_where_the_day_left_off() {
        let mut counter = InvoiceCounter::resume(id(1), 4_470);
        assert_eq!(counter.next(), Ok(4_471));
    }

    #[test]
    fn two_devices_number_independently() {
        // The reason the counter is per device: two tills offline from each other cannot share one,
        // and a shared counter would produce duplicates on the first outage.
        let mut till_one = InvoiceCounter::new(id(1));
        let mut till_two = InvoiceCounter::new(id(2));

        assert_eq!(till_one.next(), Ok(1));
        assert_eq!(till_two.next(), Ok(1));
        assert_ne!(till_one.device_id(), till_two.device_id());
    }

    #[test]
    fn exhaustion_is_refused_rather_than_wrapped() {
        let mut counter = InvoiceCounter::resume(id(1), u64::MAX);
        assert_eq!(counter.next(), Err(FiscalError::CounterExhausted));
        assert_eq!(counter.issued(), u64::MAX, "and nothing moved");
    }
}
