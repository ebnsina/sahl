use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::{Currency, Money, Rounding};
use crate::policy::lease::{ClaimVerdict, TicketLease, evaluate_claim};
use crate::tax::{self, Discount, OrderInput, OrderTotals, PricingMode};
use crate::time::Timestamp;

use super::error::SaleError;
use super::event::SaleEvent;
use super::line::{LineVoid, SaleLine};
use super::tender::{Tender, TenderMethod};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaleStatus {
    /// Live. For retail this lasts seconds; for a café ticket, an hour.
    Open,
    /// Paid and closed. Immutable.
    Completed,
    /// Dropped without payment.
    Abandoned,
}

impl SaleStatus {
    const fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
        }
    }
}

/// A sale, reconstructed from its events.
///
/// This is a **projection, not a record**. It is never stored and never mutated directly — it is
/// what you get by replaying the log, which is what makes "replay the day and prove the numbers"
/// something the product can actually do rather than claim.
///
/// Totals are never cached. They are recomputed from the lines through the VAT engine every time,
/// so a sale's displayed total and its receipt cannot drift apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sale {
    id: Uuid,
    status: SaleStatus,
    opened_by: Uuid,
    currency: Currency,
    pricing_mode: PricingMode,
    rounding: Rounding,
    lines: Vec<SaleLine>,
    order_discount: Discount,
    tenders: Vec<Tender>,
    /// Snapshotted at completion so a reprint matches the original receipt exactly, even if the
    /// engine's configuration changes later.
    settled_total: Option<Money>,
    change_given: Option<Money>,
    settled_at: Option<Timestamp>,
    /// Which device owns this ticket, if any. Only meaningful while open.
    lease: Option<TicketLease>,
}

impl Sale {
    /// Rebuild a sale from its full event history.
    ///
    /// # Errors
    /// [`SaleError`] if the log is inconsistent — a valid log always replays cleanly, so an error
    /// here means corruption or a bug, not user input.
    pub fn replay(events: &[SaleEvent]) -> Result<Self, SaleError> {
        let Some(first) = events.first() else {
            return Err(SaleError::NotOpenedFirst { found: "nothing" });
        };
        let mut sale = Self::opened(first)?;
        for event in events.iter().skip(1) {
            sale.apply(event)?;
        }
        Ok(sale)
    }

    fn opened(event: &SaleEvent) -> Result<Self, SaleError> {
        let SaleEvent::Opened {
            sale_id,
            opened_by,
            currency,
            pricing_mode,
            rounding,
        } = event
        else {
            return Err(SaleError::NotOpenedFirst {
                found: crate::event::EventPayload::kind(event),
            });
        };

        Ok(Self {
            id: *sale_id,
            status: SaleStatus::Open,
            opened_by: *opened_by,
            currency: *currency,
            pricing_mode: *pricing_mode,
            rounding: *rounding,
            lines: Vec::new(),
            order_discount: Discount::None,
            tenders: Vec::new(),
            settled_total: None,
            change_given: None,
            settled_at: None,
            lease: None,
        })
    }

    /// Apply one event.
    ///
    /// # Errors
    /// [`SaleError`] if the event is not valid for the current state.
    pub fn apply(&mut self, event: &SaleEvent) -> Result<(), SaleError> {
        if event.sale_id() != self.id {
            return Err(SaleError::WrongSale {
                expected: self.id,
                found: event.sale_id(),
            });
        }

        // Everything except the terminal transitions requires an open sale. Checked once here so no
        // individual arm can forget it.
        if !matches!(
            event,
            SaleEvent::Completed { .. } | SaleEvent::Abandoned { .. }
        ) && self.status != SaleStatus::Open
        {
            return Err(SaleError::NotOpen {
                status: self.status.label(),
            });
        }

        match event {
            SaleEvent::Opened { .. } => return Err(SaleError::AlreadyOpened),

            SaleEvent::LineAdded {
                line_id,
                product_id,
                name,
                unit_price,
                quantity,
                tax_class,
                ..
            } => {
                if self.find_line(*line_id).is_some() {
                    return Err(SaleError::DuplicateLine { line_id: *line_id });
                }
                self.lines.push(SaleLine {
                    id: *line_id,
                    product_id: *product_id,
                    name: name.clone(),
                    unit_price: *unit_price,
                    quantity: *quantity,
                    tax_class: *tax_class,
                    discount: Discount::None,
                    void: None,
                });
            }

            SaleEvent::LineQuantityChanged {
                line_id, quantity, ..
            } => {
                self.line_mut(*line_id)?.quantity = *quantity;
            }

            SaleEvent::LineDiscounted {
                line_id, discount, ..
            } => {
                self.line_mut(*line_id)?.discount = *discount;
            }

            SaleEvent::LineVoided {
                line_id,
                reason,
                authorized_by,
                ..
            } => {
                let line = self.line_mut(*line_id)?;
                if line.void.is_some() {
                    return Err(SaleError::AlreadyVoided { line_id: *line_id });
                }
                line.void = Some(LineVoid {
                    reason: *reason,
                    authorized_by: *authorized_by,
                });
            }

            SaleEvent::TicketClaimed {
                sale_id,
                device_id,
                at,
            } => {
                // Claims are recorded unconditionally. Refusing one here would mean a valid log
                // failed to replay on the server, which is where two contested claims necessarily
                // meet; `resolve_contest` decides the winner, not this.
                let claim = TicketLease::new(*sale_id, *device_id, *at);
                self.lease = Some(match self.lease {
                    Some(held) if held.holder != *device_id => {
                        crate::policy::lease::resolve_contest(&held, &claim)
                    }
                    _ => claim,
                });
            }

            SaleEvent::TicketReleased { device_id, .. } => {
                // Only the holder can release. A release from anyone else is a stale message that
                // arrived after they already lost the ticket.
                if self.lease.is_some_and(|held| held.holder == *device_id) {
                    self.lease = None;
                }
            }

            SaleEvent::OrderDiscounted { discount, .. } => {
                self.order_discount = *discount;
            }

            SaleEvent::TenderRecorded {
                tender_id,
                method,
                amount,
                reference,
                ..
            } => {
                if !amount.is_positive() {
                    return Err(SaleError::NonPositiveTender { amount: *amount });
                }
                self.tenders.push(Tender {
                    id: *tender_id,
                    method: *method,
                    amount: *amount,
                    reference: reference.clone(),
                });
                self.assert_non_cash_within_total()?;
            }

            SaleEvent::Completed {
                total,
                change_given,
                at,
                ..
            } => {
                if self.status != SaleStatus::Open {
                    return Err(SaleError::NotOpen {
                        status: self.status.label(),
                    });
                }

                // Recompute rather than trust the event. The terminal that wrote it and the server
                // replaying it run the same engine, so a mismatch means tampering or a version skew
                // — either way, not something to silently accept into a merchant's books.
                let calculated = self.totals()?.total;
                if calculated != *total {
                    return Err(SaleError::TotalMismatch {
                        recorded: *total,
                        calculated,
                    });
                }

                let outstanding = self.balance_due()?;
                if outstanding.is_positive() {
                    return Err(SaleError::Outstanding { outstanding });
                }

                let calculated_change = self.change_due()?;
                if calculated_change != *change_given {
                    return Err(SaleError::ChangeMismatch {
                        recorded: *change_given,
                        calculated: calculated_change,
                    });
                }

                self.settled_total = Some(*total);
                self.change_given = Some(*change_given);
                self.settled_at = Some(*at);
                self.status = SaleStatus::Completed;
            }

            SaleEvent::Abandoned { .. } => {
                if self.status != SaleStatus::Open {
                    return Err(SaleError::NotOpen {
                        status: self.status.label(),
                    });
                }
                self.status = SaleStatus::Abandoned;
            }
        }

        Ok(())
    }

    /// Calculate the sale through the VAT engine, excluding voided lines.
    ///
    /// # Errors
    /// [`SaleError::NoActiveLines`] on an empty sale, or a tax error.
    pub fn totals(&self) -> Result<OrderTotals, SaleError> {
        let lines: Vec<_> = self
            .lines
            .iter()
            .filter(|line| line.is_active())
            .map(SaleLine::to_tax_input)
            .collect();

        if lines.is_empty() {
            return Err(SaleError::NoActiveLines);
        }

        let mut order =
            OrderInput::new(self.currency, lines).with_order_discount(self.order_discount);
        order.pricing_mode = self.pricing_mode;
        order.rounding = self.rounding;

        Ok(tax::calculate(&order)?)
    }

    /// Everything handed over so far.
    ///
    /// # Errors
    /// [`SaleError::Money`] on overflow.
    pub fn tendered(&self) -> Result<Money, SaleError> {
        Ok(Money::try_sum(
            self.tenders.iter().map(|tender| tender.amount),
            self.currency,
        )?)
    }

    /// What is still owed. Zero or negative means the sale can close.
    ///
    /// # Errors
    /// [`SaleError`] if the sale has no active lines or arithmetic overflows.
    pub fn balance_due(&self) -> Result<Money, SaleError> {
        let total = self.totals()?.total;
        Ok(total.checked_sub(self.tendered()?)?)
    }

    /// Cash to hand back. Never negative.
    ///
    /// # Errors
    /// [`SaleError`] if the sale has no active lines or arithmetic overflows.
    pub fn change_due(&self) -> Result<Money, SaleError> {
        let balance = self.balance_due()?;
        if balance.is_negative() {
            Ok(balance.checked_neg()?)
        } else {
            Ok(Money::zero(self.currency))
        }
    }

    /// Cash that should be in the drawer from this sale — cash tendered, less change handed back.
    ///
    /// This is what a shift close reconciles against, so it deliberately counts only cash: card and
    /// wallet takings never touch the till.
    ///
    /// # Errors
    /// [`SaleError`] on overflow.
    pub fn net_cash(&self) -> Result<Money, SaleError> {
        let cash = Money::try_sum(
            self.tenders
                .iter()
                .filter(|tender| tender.method.affects_cash_drawer())
                .map(|tender| tender.amount),
            self.currency,
        )?;
        let change = self
            .change_given
            .unwrap_or_else(|| Money::zero(self.currency));
        Ok(cash.checked_sub(change)?)
    }

    fn assert_non_cash_within_total(&self) -> Result<(), SaleError> {
        let non_cash = Money::try_sum(
            self.tenders
                .iter()
                .filter(|tender| !tender.method.accepts_overtender())
                .map(|tender| tender.amount),
            self.currency,
        )?;

        // An empty sale has no total to compare against; the completion path rejects it separately.
        let Ok(totals) = self.totals() else {
            return Ok(());
        };

        if non_cash.minor() > totals.total.minor() {
            return Err(SaleError::NonCashOvertender {
                tendered: non_cash,
                total: totals.total,
            });
        }
        Ok(())
    }

    fn find_line(&self, line_id: Uuid) -> Option<&SaleLine> {
        self.lines.iter().find(|line| line.id == line_id)
    }

    fn line_mut(&mut self, line_id: Uuid) -> Result<&mut SaleLine, SaleError> {
        self.lines
            .iter_mut()
            .find(|line| line.id == line_id)
            .ok_or(SaleError::UnknownLine { line_id })
    }

    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.id
    }

    /// Who holds this ticket, if anyone.
    #[must_use]
    pub const fn lease(&self) -> Option<TicketLease> {
        self.lease
    }

    /// Whether `device` may append to this ticket.
    ///
    /// Consulted before writing, not during replay: replay must accept whatever actually happened,
    /// including a contest that should never have occurred.
    #[must_use]
    pub fn may_write(&self, device: Uuid, now: Timestamp) -> ClaimVerdict {
        evaluate_claim(self.lease.as_ref(), device, now)
    }

    #[must_use]
    pub const fn status(&self) -> SaleStatus {
        self.status
    }

    #[must_use]
    pub const fn opened_by(&self) -> Uuid {
        self.opened_by
    }

    #[must_use]
    pub const fn currency(&self) -> Currency {
        self.currency
    }

    /// Every line, including voided ones. Voids are evidence and are never hidden from a caller
    /// that asks for the full picture.
    #[must_use]
    pub fn lines(&self) -> &[SaleLine] {
        &self.lines
    }

    pub fn active_lines(&self) -> impl Iterator<Item = &SaleLine> {
        self.lines.iter().filter(|line| line.is_active())
    }

    #[must_use]
    pub fn tenders(&self) -> &[Tender] {
        &self.tenders
    }

    /// The total as recorded at completion, if the sale is closed.
    #[must_use]
    pub const fn settled_total(&self) -> Option<Money> {
        self.settled_total
    }

    #[must_use]
    pub const fn change_given(&self) -> Option<Money> {
        self.change_given
    }

    /// When the sale closed — what attributes it to a shift.
    #[must_use]
    pub const fn settled_at(&self) -> Option<Timestamp> {
        self.settled_at
    }

    /// How many lines were voided — the raw material for the void-rate anomaly signal.
    #[must_use]
    pub fn void_count(&self) -> usize {
        self.lines.iter().filter(|line| !line.is_active()).count()
    }

    /// Whether a cash drawer should open on completion.
    #[must_use]
    pub fn needs_drawer(&self) -> bool {
        self.tenders
            .iter()
            .any(|tender| tender.method.affects_cash_drawer())
    }
}

/// Convenience for the common cash path.
#[must_use]
pub const fn cash() -> TenderMethod {
    TenderMethod::Cash
}
