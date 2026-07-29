//! Stock transfers between outlets, rebuilt from events.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::money::MoneyError;
use crate::quantity::Quantity;
use crate::time::Timestamp;

use super::event::{DispatchLine, TransferEvent};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransferError {
    #[error("arithmetic error: {0}")]
    Money(#[from] MoneyError),

    #[error("transfer {transfer_id} was never dispatched")]
    NotDispatched { transfer_id: Uuid },

    #[error("transfer {transfer_id} was already dispatched")]
    AlreadyDispatched { transfer_id: Uuid },

    #[error("transfer {transfer_id} has no line {line_id}")]
    UnknownLine { transfer_id: Uuid, line_id: Uuid },

    #[error("transfer {transfer_id} is settled; nothing more may be received against it")]
    Settled { transfer_id: Uuid },

    #[error("a receipt of {quantity} is not a positive amount")]
    NonPositiveReceipt { quantity: Quantity },

    #[error("a transfer must move at least one line")]
    Empty,

    #[error("an outlet cannot transfer to itself")]
    SameOutlet,
}

/// A dispatched line and what turned up at the other end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineTransit {
    pub line: DispatchLine,
    pub received: Quantity,
}

impl LineTransit {
    /// Sent minus received. Positive means stock is still in transit or lost.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow.
    pub fn shortfall(&self) -> Result<Quantity, MoneyError> {
        self.line.quantity.checked_add(self.received.checked_neg()?)
    }

    /// Whether everything sent has arrived.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow.
    pub fn is_arrived(&self) -> Result<bool, MoneyError> {
        Ok(!self.shortfall()?.milli().is_positive())
    }
}

/// Where a transfer has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    /// Sent, nothing has arrived. The stock is in neither outlet's stock on hand.
    InTransit,
    /// Some of it has arrived.
    PartlyArrived,
    /// All of it has arrived, but nobody has settled the transfer.
    Arrived,
    /// Settled, with the shortfall it was settled at.
    Settled { short: bool },
}

/// One transfer between two outlets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub transfer_id: Uuid,
    pub from_outlet: Uuid,
    pub to_outlet: Uuid,
    pub dispatched_at: Timestamp,
    pub dispatched_by: Uuid,
    lines: BTreeMap<Uuid, LineTransit>,
    settled: bool,
}

impl Transfer {
    /// Rebuild from a stream of events for one transfer.
    ///
    /// # Errors
    /// [`TransferError`] if the stream is inconsistent.
    pub fn replay(events: &[TransferEvent]) -> Result<Self, TransferError> {
        let mut transfer = match events.first() {
            Some(TransferEvent::Dispatched {
                transfer_id,
                from_outlet,
                to_outlet,
                lines,
                at,
                dispatched_by,
            }) => {
                if lines.is_empty() {
                    return Err(TransferError::Empty);
                }
                if from_outlet == to_outlet {
                    return Err(TransferError::SameOutlet);
                }
                Self {
                    transfer_id: *transfer_id,
                    from_outlet: *from_outlet,
                    to_outlet: *to_outlet,
                    dispatched_at: *at,
                    dispatched_by: *dispatched_by,
                    lines: lines
                        .iter()
                        .map(|line| {
                            (
                                line.line_id,
                                LineTransit {
                                    line: line.clone(),
                                    received: Quantity::from_milli(0),
                                },
                            )
                        })
                        .collect(),
                    settled: false,
                }
            }
            Some(other) => {
                return Err(TransferError::NotDispatched {
                    transfer_id: other.transfer_id(),
                });
            }
            None => return Err(TransferError::Empty),
        };

        for event in events.iter().skip(1) {
            transfer.apply(event)?;
        }
        Ok(transfer)
    }

    /// Apply one event.
    ///
    /// # Errors
    /// [`TransferError`] if the event is not valid for the current state.
    pub fn apply(&mut self, event: &TransferEvent) -> Result<(), TransferError> {
        if self.settled {
            return Err(TransferError::Settled {
                transfer_id: self.transfer_id,
            });
        }

        match event {
            TransferEvent::Dispatched { transfer_id, .. } => {
                return Err(TransferError::AlreadyDispatched {
                    transfer_id: *transfer_id,
                });
            }

            TransferEvent::Received {
                line_id, quantity, ..
            } => {
                if !quantity.milli().is_positive() {
                    return Err(TransferError::NonPositiveReceipt {
                        quantity: *quantity,
                    });
                }
                let transfer_id = self.transfer_id;
                let transit = self
                    .lines
                    .get_mut(line_id)
                    .ok_or(TransferError::UnknownLine {
                        transfer_id,
                        line_id: *line_id,
                    })?;
                transit.received = transit.received.checked_add(*quantity)?;
            }

            TransferEvent::Settled { .. } => self.settled = true,
        }

        Ok(())
    }

    /// Where the transfer has got to.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow.
    pub fn status(&self) -> Result<TransferStatus, MoneyError> {
        let mut any_received = false;
        let mut all_arrived = true;
        for transit in self.lines.values() {
            if !transit.received.is_zero() {
                any_received = true;
            }
            if !transit.is_arrived()? {
                all_arrived = false;
            }
        }

        if self.settled {
            return Ok(TransferStatus::Settled {
                short: !all_arrived,
            });
        }
        Ok(match (any_received, all_arrived) {
            (_, true) => TransferStatus::Arrived,
            (true, false) => TransferStatus::PartlyArrived,
            (false, false) => TransferStatus::InTransit,
        })
    }

    /// Stock that left but has not arrived, per line.
    ///
    /// While a transfer is open this is legitimately in-transit; once it is settled the same number
    /// is a loss. The distinction is [`Transfer::status`], not this — an owner wants the quantity
    /// either way.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow.
    pub fn in_transit(&self) -> Result<Vec<(&LineTransit, Quantity)>, MoneyError> {
        let mut outstanding = Vec::new();
        for transit in self.lines.values() {
            let short = transit.shortfall()?;
            if short.milli().is_positive() {
                outstanding.push((transit, short));
            }
        }
        Ok(outstanding)
    }

    /// Whether more arrived than was sent, on any line.
    ///
    /// Means the dispatch was recorded wrong, not that stock multiplied — worth flagging because it
    /// leaves the sending outlet's book overstated.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow.
    pub fn has_over_receipt(&self) -> Result<bool, MoneyError> {
        for transit in self.lines.values() {
            if transit.shortfall()?.is_negative() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[must_use]
    pub fn line(&self, line_id: Uuid) -> Option<&LineTransit> {
        self.lines.get(&line_id)
    }

    #[must_use]
    pub fn lines(&self) -> Vec<&LineTransit> {
        self.lines.values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(day: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + day * 86_400_000)
    }

    fn qty(milli: i64) -> Quantity {
        Quantity::from_milli(milli)
    }

    const DHANMONDI: u128 = 0xD1;
    const GULSHAN: u128 = 0x62;
    const STAFF: u128 = 0xCA;

    fn line(n: u128, milli: i64) -> DispatchLine {
        DispatchLine {
            line_id: id(n),
            product_id: id(0x100 + n),
            batch_id: id(0x200 + n),
            quantity: qty(milli),
        }
    }

    fn dispatched(lines: Vec<DispatchLine>) -> TransferEvent {
        TransferEvent::Dispatched {
            transfer_id: id(1),
            from_outlet: id(DHANMONDI),
            to_outlet: id(GULSHAN),
            lines,
            at: at(0),
            dispatched_by: id(STAFF),
        }
    }

    fn received(line_id: u128, milli: i64, day: i64) -> TransferEvent {
        TransferEvent::Received {
            transfer_id: id(1),
            line_id: id(line_id),
            batch_id: id(0x300 + line_id),
            quantity: qty(milli),
            at: at(day),
            received_by: id(STAFF),
        }
    }

    #[test]
    fn dispatched_stock_is_in_neither_outlet() {
        // The reason a transfer is two events. Pretending it lands instantly means one outlet's
        // count is wrong for as long as the van is on the road.
        let transfer = Transfer::replay(&[dispatched(vec![line(1, 10_000)])]).expect("valid");

        assert_eq!(transfer.status(), Ok(TransferStatus::InTransit));
        assert_eq!(transfer.in_transit().expect("computes").len(), 1);
    }

    #[test]
    fn arrival_closes_the_gap() {
        let transfer =
            Transfer::replay(&[dispatched(vec![line(1, 10_000)]), received(1, 10_000, 1)])
                .expect("valid");

        assert_eq!(transfer.status(), Ok(TransferStatus::Arrived));
        assert!(transfer.in_transit().expect("computes").is_empty());
    }

    #[test]
    fn a_partial_arrival_leaves_the_rest_in_transit() {
        let transfer =
            Transfer::replay(&[dispatched(vec![line(1, 10_000)]), received(1, 6_000, 1)])
                .expect("valid");

        assert_eq!(transfer.status(), Ok(TransferStatus::PartlyArrived));
        assert_eq!(
            transfer.line(id(1)).expect("present").shortfall(),
            Ok(qty(4_000))
        );
    }

    #[test]
    fn settling_short_records_the_loss_rather_than_erasing_it() {
        // Ten crates left, nine arrived. That gap is the number a two-outlet owner is looking for,
        // and settling must not quietly reconcile it away.
        let transfer = Transfer::replay(&[
            dispatched(vec![line(1, 10_000)]),
            received(1, 9_000, 1),
            TransferEvent::Settled {
                transfer_id: id(1),
                at: at(2),
                settled_by: id(STAFF),
            },
        ])
        .expect("valid");

        assert_eq!(
            transfer.status(),
            Ok(TransferStatus::Settled { short: true })
        );
        assert_eq!(transfer.in_transit().expect("computes")[0].1, qty(1_000));
    }

    #[test]
    fn settling_complete_is_distinguishable_from_settling_short() {
        let transfer = Transfer::replay(&[
            dispatched(vec![line(1, 10_000)]),
            received(1, 10_000, 1),
            TransferEvent::Settled {
                transfer_id: id(1),
                at: at(2),
                settled_by: id(STAFF),
            },
        ])
        .expect("valid");

        assert_eq!(
            transfer.status(),
            Ok(TransferStatus::Settled { short: false })
        );
    }

    #[test]
    fn more_arriving_than_left_is_flagged() {
        // Stock did not multiply — the dispatch was recorded wrong, and the sending outlet's book
        // is now overstated.
        let transfer =
            Transfer::replay(&[dispatched(vec![line(1, 10_000)]), received(1, 11_000, 1)])
                .expect("valid");

        assert_eq!(transfer.has_over_receipt(), Ok(true));
    }

    #[test]
    fn a_transfer_to_the_same_outlet_is_refused() {
        let result = Transfer::replay(&[TransferEvent::Dispatched {
            transfer_id: id(1),
            from_outlet: id(DHANMONDI),
            to_outlet: id(DHANMONDI),
            lines: vec![line(1, 10_000)],
            at: at(0),
            dispatched_by: id(STAFF),
        }]);

        assert_eq!(result, Err(TransferError::SameOutlet));
    }

    #[test]
    fn nothing_may_arrive_against_a_settled_transfer() {
        let result = Transfer::replay(&[
            dispatched(vec![line(1, 10_000)]),
            TransferEvent::Settled {
                transfer_id: id(1),
                at: at(2),
                settled_by: id(STAFF),
            },
            received(1, 10_000, 3),
        ]);

        assert_eq!(result, Err(TransferError::Settled { transfer_id: id(1) }));
    }

    #[test]
    fn a_stream_that_does_not_start_with_a_dispatch_is_refused() {
        let result = Transfer::replay(&[received(1, 1_000, 1)]);
        assert_eq!(
            result,
            Err(TransferError::NotDispatched { transfer_id: id(1) })
        );
    }

    #[test]
    fn an_empty_transfer_is_refused() {
        assert_eq!(Transfer::replay(&[]), Err(TransferError::Empty));
        assert_eq!(
            Transfer::replay(&[dispatched(Vec::new())]),
            Err(TransferError::Empty)
        );
    }

    #[test]
    fn replay_is_deterministic() {
        let events = vec![
            dispatched(vec![line(1, 10_000), line(2, 4_000)]),
            received(2, 4_000, 1),
            received(1, 9_000, 2),
        ];

        assert_eq!(
            Transfer::replay(&events).expect("valid"),
            Transfer::replay(&events).expect("valid")
        );
    }
}
