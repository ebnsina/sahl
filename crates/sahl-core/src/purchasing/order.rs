//! Purchase order state, rebuilt from events.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::money::{Money, MoneyError, Rounding};
use crate::quantity::Quantity;
use crate::time::Timestamp;

use super::event::{CloseReason, OrderLine, PurchaseEvent};

/// Extend a unit cost across a quantity.
///
/// `HalfUp` to match the tax engine — a purchase line and a sale line of the same weight must round
/// the same way, or margin on a weighed product drifts by a paisa per transaction.
fn extend(unit_cost: Money, quantity: Quantity) -> Result<Money, MoneyError> {
    unit_cost.mul_ratio(quantity.milli(), Quantity::MILLI_PER_UNIT, Rounding::HalfUp)
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PurchaseError {
    #[error("arithmetic error: {0}")]
    Money(#[from] MoneyError),

    #[error("order {order_id} was never placed")]
    NotPlaced { order_id: Uuid },

    #[error("order {order_id} was already placed")]
    AlreadyPlaced { order_id: Uuid },

    #[error("order {order_id} has no line {line_id}")]
    UnknownLine { order_id: Uuid, line_id: Uuid },

    #[error("order {order_id} is closed; nothing more may be received against it")]
    Closed { order_id: Uuid },

    #[error("a receipt of {quantity} is not a positive amount")]
    NonPositiveReceipt { quantity: Quantity },

    #[error("an order must have at least one line")]
    Empty,
}

/// A line, with what has arrived against it so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineProgress {
    pub line: OrderLine,
    pub received: Quantity,
    /// What was actually paid across every receipt on this line.
    pub received_value: Money,
}

impl LineProgress {
    /// Ordered minus received. Negative means the supplier sent more than was asked for.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow.
    pub fn outstanding(&self) -> Result<Quantity, MoneyError> {
        self.line.quantity.checked_add(self.received.checked_neg()?)
    }

    /// Whether everything ordered has arrived.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow.
    pub fn is_fulfilled(&self) -> Result<bool, MoneyError> {
        Ok(!self.outstanding()?.milli().is_positive())
    }
}

/// Where an order has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    /// Placed, nothing has arrived.
    Awaiting,
    /// Some of it has arrived.
    PartlyReceived,
    /// All of it has arrived, but nobody has closed the order.
    FullyReceived,
    /// Done with, for the recorded reason.
    Closed(CloseReason),
}

/// One purchase order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseOrder {
    pub order_id: Uuid,
    pub supplier: String,
    pub reference: Option<String>,
    pub expected_at: Option<Timestamp>,
    pub placed_at: Timestamp,
    pub placed_by: Uuid,
    /// Keyed by line id, so ordering is stable across processes.
    lines: BTreeMap<Uuid, LineProgress>,
    closed: Option<CloseReason>,
}

impl PurchaseOrder {
    /// Rebuild from a stream of events for one order.
    ///
    /// # Errors
    /// [`PurchaseError`] if the stream is inconsistent.
    pub fn replay(events: &[PurchaseEvent]) -> Result<Self, PurchaseError> {
        let mut order = match events.first() {
            Some(PurchaseEvent::Placed {
                order_id,
                supplier,
                reference,
                lines,
                expected_at,
                at,
                placed_by,
            }) => {
                if lines.is_empty() {
                    return Err(PurchaseError::Empty);
                }
                Self {
                    order_id: *order_id,
                    supplier: supplier.clone(),
                    reference: reference.clone(),
                    expected_at: *expected_at,
                    placed_at: *at,
                    placed_by: *placed_by,
                    lines: lines
                        .iter()
                        .map(|line| {
                            (
                                line.line_id,
                                LineProgress {
                                    line: line.clone(),
                                    received: Quantity::from_milli(0),
                                    received_value: Money::from_minor(0, line.unit_cost.currency()),
                                },
                            )
                        })
                        .collect(),
                    closed: None,
                }
            }
            Some(other) => {
                return Err(PurchaseError::NotPlaced {
                    order_id: other.order_id(),
                });
            }
            None => return Err(PurchaseError::Empty),
        };

        for event in events.iter().skip(1) {
            order.apply(event)?;
        }
        Ok(order)
    }

    /// Apply one event.
    ///
    /// # Errors
    /// [`PurchaseError`] if the event is not valid for the current state.
    pub fn apply(&mut self, event: &PurchaseEvent) -> Result<(), PurchaseError> {
        if self.closed.is_some() {
            return Err(PurchaseError::Closed {
                order_id: self.order_id,
            });
        }

        match event {
            PurchaseEvent::Placed { order_id, .. } => {
                return Err(PurchaseError::AlreadyPlaced {
                    order_id: *order_id,
                });
            }

            PurchaseEvent::LineReceived {
                line_id,
                quantity,
                unit_cost,
                ..
            } => {
                if !quantity.milli().is_positive() {
                    return Err(PurchaseError::NonPositiveReceipt {
                        quantity: *quantity,
                    });
                }
                let order_id = self.order_id;
                let progress = self
                    .lines
                    .get_mut(line_id)
                    .ok_or(PurchaseError::UnknownLine {
                        order_id,
                        line_id: *line_id,
                    })?;

                // Over-receipt is recorded, not refused. A supplier sending eleven when ten were
                // ordered is a real thing that happens, and a book that will not record it is a
                // book that disagrees with the shelf.
                progress.received = progress.received.checked_add(*quantity)?;
                progress.received_value = progress
                    .received_value
                    .checked_add(extend(*unit_cost, *quantity)?)?;
            }

            PurchaseEvent::Closed { reason, .. } => self.closed = Some(*reason),
        }

        Ok(())
    }

    /// Where the order has got to.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow.
    pub fn status(&self) -> Result<OrderStatus, MoneyError> {
        if let Some(reason) = self.closed {
            return Ok(OrderStatus::Closed(reason));
        }
        let mut any_received = false;
        let mut all_fulfilled = true;
        for progress in self.lines.values() {
            if !progress.received.is_zero() {
                any_received = true;
            }
            if !progress.is_fulfilled()? {
                all_fulfilled = false;
            }
        }
        Ok(match (any_received, all_fulfilled) {
            (_, true) => OrderStatus::FullyReceived,
            (true, false) => OrderStatus::PartlyReceived,
            (false, false) => OrderStatus::Awaiting,
        })
    }

    /// Lines still waiting on stock, in line-id order.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow.
    pub fn outstanding(&self) -> Result<Vec<&LineProgress>, MoneyError> {
        let mut waiting = Vec::new();
        for progress in self.lines.values() {
            if !progress.is_fulfilled()? {
                waiting.push(progress);
            }
        }
        Ok(waiting)
    }

    /// What the order said it would cost.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow or a mixed-currency order.
    pub fn ordered_value(&self) -> Result<Money, MoneyError> {
        let mut total = Money::from_minor(0, self.currency());
        for progress in self.lines.values() {
            total = total.checked_add(extend(progress.line.unit_cost, progress.line.quantity)?)?;
        }
        Ok(total)
    }

    /// What has actually been charged so far.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow or a mixed-currency order.
    pub fn received_value(&self) -> Result<Money, MoneyError> {
        let mut total = Money::from_minor(0, self.currency());
        for progress in self.lines.values() {
            total = total.checked_add(progress.received_value)?;
        }
        Ok(total)
    }

    /// Lines where the price charged did not match the price ordered.
    ///
    /// The reason to keep an order document at all — a supplier who raises a price quietly between
    /// quote and delivery is invisible without something to compare against.
    ///
    /// # Errors
    /// [`MoneyError`] on overflow.
    pub fn price_discrepancies(&self) -> Result<Vec<&LineProgress>, MoneyError> {
        let mut found = Vec::new();
        for progress in self.lines.values() {
            if progress.received.is_zero() {
                continue;
            }
            let expected = extend(progress.line.unit_cost, progress.received)?;
            if expected != progress.received_value {
                found.push(progress);
            }
        }
        Ok(found)
    }

    #[must_use]
    pub fn line(&self, line_id: Uuid) -> Option<&LineProgress> {
        self.lines.get(&line_id)
    }

    #[must_use]
    pub fn lines(&self) -> Vec<&LineProgress> {
        self.lines.values().collect()
    }

    /// The order's currency, taken from its first line. Mixed currencies are rejected downstream by
    /// `Money`'s own arithmetic rather than checked twice here.
    fn currency(&self) -> crate::money::Currency {
        self.lines
            .values()
            .next()
            .map_or(crate::money::Currency::Bdt, |progress| {
                progress.line.unit_cost.currency()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(day: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + day * 86_400_000)
    }

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, Currency::Bdt)
    }

    fn qty(milli: i64) -> Quantity {
        Quantity::from_milli(milli)
    }

    const STAFF: u128 = 0xCA;

    fn placed(lines: Vec<OrderLine>) -> PurchaseEvent {
        PurchaseEvent::Placed {
            order_id: id(1),
            supplier: "Karim Traders".to_owned(),
            reference: Some("KT-4471".to_owned()),
            lines,
            expected_at: Some(at(3)),
            at: at(0),
            placed_by: id(STAFF),
        }
    }

    fn line(n: u128, milli: i64, cost: i64) -> OrderLine {
        OrderLine {
            line_id: id(n),
            product_id: id(0x100 + n),
            quantity: qty(milli),
            unit_cost: bdt(cost),
        }
    }

    fn received(line_id: u128, milli: i64, cost: i64, day: i64) -> PurchaseEvent {
        PurchaseEvent::LineReceived {
            order_id: id(1),
            line_id: id(line_id),
            batch_id: id(0x900 + line_id),
            quantity: qty(milli),
            unit_cost: bdt(cost),
            at: at(day),
            received_by: id(STAFF),
        }
    }

    #[test]
    fn a_placed_order_is_awaiting_everything() {
        let order = PurchaseOrder::replay(&[placed(vec![line(1, 10_000, 4_000)])]).expect("valid");

        assert_eq!(order.status(), Ok(OrderStatus::Awaiting));
        assert_eq!(order.outstanding().expect("computes").len(), 1);
        assert_eq!(order.ordered_value(), Ok(bdt(40_000)));
    }

    #[test]
    fn a_part_delivery_leaves_the_rest_outstanding() {
        // Part deliveries are the norm. An order that could only be received in full would push
        // staff into recording it as one lump on the day the last box turns up.
        let order = PurchaseOrder::replay(&[
            placed(vec![line(1, 10_000, 4_000)]),
            received(1, 6_000, 4_000, 3),
        ])
        .expect("valid");

        assert_eq!(order.status(), Ok(OrderStatus::PartlyReceived));
        assert_eq!(
            order.line(id(1)).expect("present").outstanding(),
            Ok(qty(4_000))
        );
    }

    #[test]
    fn several_deliveries_against_one_line_accumulate() {
        let order = PurchaseOrder::replay(&[
            placed(vec![line(1, 10_000, 4_000)]),
            received(1, 6_000, 4_000, 3),
            received(1, 4_000, 4_000, 5),
        ])
        .expect("valid");

        assert_eq!(order.status(), Ok(OrderStatus::FullyReceived));
        assert_eq!(order.received_value(), Ok(bdt(40_000)));
    }

    #[test]
    fn an_over_delivery_is_recorded_not_refused() {
        // A book that will not record eleven when ten were ordered is a book that disagrees with
        // the shelf, and the shelf is the authority.
        let order = PurchaseOrder::replay(&[
            placed(vec![line(1, 10_000, 4_000)]),
            received(1, 11_000, 4_000, 3),
        ])
        .expect("valid");

        assert_eq!(order.status(), Ok(OrderStatus::FullyReceived));
        assert_eq!(
            order.line(id(1)).expect("present").outstanding(),
            Ok(qty(-1_000))
        );
    }

    #[test]
    fn a_price_that_changed_between_quote_and_delivery_is_surfaced() {
        // The entire reason to keep an order document.
        let order = PurchaseOrder::replay(&[
            placed(vec![line(1, 10_000, 4_000), line(2, 5_000, 9_000)]),
            received(1, 10_000, 4_400, 3),
            received(2, 5_000, 9_000, 3),
        ])
        .expect("valid");

        let discrepancies = order.price_discrepancies().expect("computes");
        assert_eq!(discrepancies.len(), 1);
        assert_eq!(discrepancies[0].line.line_id, id(1));
        assert_eq!(order.ordered_value(), Ok(bdt(85_000)));
        assert_eq!(
            order.received_value(),
            Ok(bdt(89_000)),
            "4,000 more than quoted"
        );
    }

    #[test]
    fn a_short_shipped_order_closes_with_the_shortfall_intact() {
        // Closing settles the expectation; it does not pretend the stock arrived.
        let order = PurchaseOrder::replay(&[
            placed(vec![line(1, 10_000, 4_000)]),
            received(1, 6_000, 4_000, 3),
            PurchaseEvent::Closed {
                order_id: id(1),
                reason: CloseReason::ShortShipped,
                at: at(9),
                closed_by: id(STAFF),
            },
        ])
        .expect("valid");

        assert_eq!(
            order.status(),
            Ok(OrderStatus::Closed(CloseReason::ShortShipped))
        );
        assert_eq!(
            order.line(id(1)).expect("present").outstanding(),
            Ok(qty(4_000))
        );
    }

    #[test]
    fn nothing_may_be_received_against_a_closed_order() {
        let result = PurchaseOrder::replay(&[
            placed(vec![line(1, 10_000, 4_000)]),
            PurchaseEvent::Closed {
                order_id: id(1),
                reason: CloseReason::Cancelled,
                at: at(1),
                closed_by: id(STAFF),
            },
            received(1, 6_000, 4_000, 3),
        ]);

        assert_eq!(result, Err(PurchaseError::Closed { order_id: id(1) }));
    }

    #[test]
    fn a_receipt_against_a_line_that_was_never_ordered_is_refused() {
        let result = PurchaseOrder::replay(&[
            placed(vec![line(1, 10_000, 4_000)]),
            received(7, 1_000, 4_000, 3),
        ]);

        assert_eq!(
            result,
            Err(PurchaseError::UnknownLine {
                order_id: id(1),
                line_id: id(7)
            })
        );
    }

    #[test]
    fn a_stream_that_does_not_start_with_a_placement_is_refused() {
        let result = PurchaseOrder::replay(&[received(1, 1_000, 4_000, 3)]);
        assert_eq!(result, Err(PurchaseError::NotPlaced { order_id: id(1) }));
    }

    #[test]
    fn an_empty_order_is_refused() {
        assert_eq!(PurchaseOrder::replay(&[]), Err(PurchaseError::Empty));
        assert_eq!(
            PurchaseOrder::replay(&[placed(Vec::new())]),
            Err(PurchaseError::Empty)
        );
    }

    #[test]
    fn a_zero_receipt_is_refused() {
        let result = PurchaseOrder::replay(&[
            placed(vec![line(1, 10_000, 4_000)]),
            received(1, 0, 4_000, 3),
        ]);
        assert!(matches!(
            result,
            Err(PurchaseError::NonPositiveReceipt { .. })
        ));
    }

    #[test]
    fn replay_is_deterministic() {
        // The terminal and the server both do this and must agree on every figure.
        let events = vec![
            placed(vec![line(1, 10_000, 4_000), line(2, 5_000, 9_000)]),
            received(2, 5_000, 9_100, 3),
            received(1, 10_000, 4_000, 4),
        ];

        assert_eq!(
            PurchaseOrder::replay(&events).expect("valid"),
            PurchaseOrder::replay(&events).expect("valid")
        );
    }
}
