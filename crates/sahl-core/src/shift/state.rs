use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::money::{Currency, Money};
use crate::sale::Sale;
use crate::time::Timestamp;

use super::event::{CashMovementReason, ShiftEvent};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ShiftError {
    #[error("money error in shift: {0}")]
    Money(#[from] crate::money::MoneyError),

    #[error("sale error in shift: {0}")]
    Sale(#[from] crate::sale::SaleError),

    #[error("the first event of a shift must be `opened`, found `{found}`")]
    NotOpenedFirst { found: &'static str },

    #[error("shift was already opened")]
    AlreadyOpened,

    #[error("event belongs to shift {found} but this is shift {expected}")]
    WrongShift { expected: Uuid, found: Uuid },

    #[error("shift is closed and can no longer be modified")]
    Closed,

    #[error("a counted drawer cannot hold {counted}")]
    NegativeCount { counted: Money },

    #[error("cannot close without counting the drawer first")]
    NotCounted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShiftStatus {
    Open,
    Closed,
}

/// A till session, from taking the drawer to handing it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shift {
    id: Uuid,
    status: ShiftStatus,
    opened_by: Uuid,
    currency: Currency,
    opening_float: Money,
    opened_at: Timestamp,
    closed_at: Option<Timestamp>,
    movements: Vec<CashMovement>,
    counts: Vec<DrawerCount>,
    closing_cash: Option<Money>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CashMovement {
    pub id: Uuid,
    pub amount: Money,
    pub reason: CashMovementReason,
    pub note: Option<String>,
    pub authorized_by: Uuid,
    pub at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawerCount {
    pub counted: Money,
    pub counted_by: Uuid,
    pub at: Timestamp,
}

impl Shift {
    /// Rebuild from the shift's events.
    ///
    /// # Errors
    /// [`ShiftError`] if the log is inconsistent.
    pub fn replay(events: &[ShiftEvent]) -> Result<Self, ShiftError> {
        let Some(first) = events.first() else {
            return Err(ShiftError::NotOpenedFirst { found: "nothing" });
        };
        let mut shift = Self::opened(first)?;
        for event in events.iter().skip(1) {
            shift.apply(event)?;
        }
        Ok(shift)
    }

    fn opened(event: &ShiftEvent) -> Result<Self, ShiftError> {
        let ShiftEvent::Opened {
            shift_id,
            opened_by,
            currency,
            opening_float,
            at,
        } = event
        else {
            return Err(ShiftError::NotOpenedFirst {
                found: crate::event::EventPayload::kind(event),
            });
        };

        Ok(Self {
            id: *shift_id,
            status: ShiftStatus::Open,
            opened_by: *opened_by,
            currency: *currency,
            opening_float: *opening_float,
            opened_at: *at,
            closed_at: None,
            movements: Vec::new(),
            counts: Vec::new(),
            closing_cash: None,
        })
    }

    /// Apply one event.
    ///
    /// # Errors
    /// [`ShiftError`] if the event is not valid for the current state.
    pub fn apply(&mut self, event: &ShiftEvent) -> Result<(), ShiftError> {
        if event.shift_id() != self.id {
            return Err(ShiftError::WrongShift {
                expected: self.id,
                found: event.shift_id(),
            });
        }
        if self.status == ShiftStatus::Closed {
            return Err(ShiftError::Closed);
        }

        match event {
            ShiftEvent::Opened { .. } => return Err(ShiftError::AlreadyOpened),

            ShiftEvent::CashMoved {
                movement_id,
                amount,
                reason,
                note,
                authorized_by,
                at,
                ..
            } => {
                self.movements.push(CashMovement {
                    id: *movement_id,
                    amount: *amount,
                    reason: *reason,
                    note: note.clone(),
                    authorized_by: *authorized_by,
                    at: *at,
                });
            }

            ShiftEvent::Counted {
                counted,
                counted_by,
                at,
                ..
            } => {
                if counted.is_negative() {
                    return Err(ShiftError::NegativeCount { counted: *counted });
                }
                // Counts accumulate rather than replace. A cashier who counts twice because the
                // first figure looked wrong leaves both attempts on the record, and a recount that
                // suddenly matches is itself worth an owner seeing.
                self.counts.push(DrawerCount {
                    counted: *counted,
                    counted_by: *counted_by,
                    at: *at,
                });
            }

            ShiftEvent::Closed {
                closing_cash, at, ..
            } => {
                if self.counts.is_empty() {
                    return Err(ShiftError::NotCounted);
                }
                self.closing_cash = Some(*closing_cash);
                self.closed_at = Some(*at);
                self.status = ShiftStatus::Closed;
            }
        }

        Ok(())
    }

    /// Net of cash moved in and out outside sales.
    ///
    /// # Errors
    /// [`ShiftError::Money`] on overflow or currency mismatch.
    pub fn net_movements(&self) -> Result<Money, ShiftError> {
        Ok(Money::try_sum(
            self.movements.iter().map(|movement| movement.amount),
            self.currency,
        )?)
    }

    /// Whether a sale belongs to this shift.
    ///
    /// By completion time, which is how tills conventionally attribute them: a sale that closes
    /// after the shift does belongs to the next one, even if it opened before.
    #[must_use]
    pub fn covers(&self, at: Timestamp) -> bool {
        if at.millis() < self.opened_at.millis() {
            return false;
        }
        self.closed_at
            .is_none_or(|closed| at.millis() <= closed.millis())
    }

    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub const fn status(&self) -> ShiftStatus {
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
    #[must_use]
    pub const fn opening_float(&self) -> Money {
        self.opening_float
    }
    #[must_use]
    pub const fn opened_at(&self) -> Timestamp {
        self.opened_at
    }
    #[must_use]
    pub const fn closed_at(&self) -> Option<Timestamp> {
        self.closed_at
    }
    #[must_use]
    pub fn movements(&self) -> &[CashMovement] {
        &self.movements
    }
    /// Every count, in order. Recounts are evidence, not noise.
    #[must_use]
    pub fn counts(&self) -> &[DrawerCount] {
        &self.counts
    }
    /// The count the drawer was closed on.
    #[must_use]
    pub fn final_count(&self) -> Option<DrawerCount> {
        self.counts.last().copied()
    }
    #[must_use]
    pub const fn closing_cash(&self) -> Option<Money> {
        self.closing_cash
    }

    /// Cash the drawer should hold, given the sales belonging to this shift.
    ///
    /// Opening float, plus cash taken in sales less change given, plus movements in and out. Only
    /// cash counts — card and wallet takings never touch the till, and including them is the
    /// classic way a shift report ends up looking short every single day.
    ///
    /// # Errors
    /// [`ShiftError`] on overflow or currency mismatch.
    pub fn expected_cash<'s, I>(&self, sales: I) -> Result<Money, ShiftError>
    where
        I: IntoIterator<Item = &'s Sale>,
    {
        let from_sales = sales
            .into_iter()
            .filter(|sale| sale.settled_at().is_some_and(|at| self.covers(at)))
            .try_fold(Money::zero(self.currency), |total, sale| {
                Ok::<_, ShiftError>(total.checked_add(sale.net_cash()?)?)
            })?;

        Ok(self
            .opening_float
            .checked_add(from_sales)?
            .checked_add(self.net_movements()?)?)
    }
}
