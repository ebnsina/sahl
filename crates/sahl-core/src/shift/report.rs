//! X and Z reports.
//!
//! An **X report** is a mid-shift snapshot: read the numbers, leave the session running. A **Z
//! report** is the close, and the figure a merchant reconciles against.
//!
//! The variance is the point. A drawer that is short every evening by roughly the same amount is a
//! very different conversation from one that is short once by a lot, and neither is visible without
//! comparing a physical count against what the events say should be there.

use serde::{Deserialize, Serialize};

use crate::money::Money;
use crate::sale::Sale;

use super::state::{Shift, ShiftError, ShiftStatus};

/// How the drawer compares with expectation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Variance {
    /// Counted exactly what was expected.
    Balanced,
    /// Less in the drawer than there should be.
    Short { by: Money },
    /// More in the drawer than there should be.
    ///
    /// Not automatically good news: a consistent over usually means sales are going unrecorded and
    /// the cash is arriving anyway.
    Over { by: Money },
}

impl Variance {
    /// Compare a count against expectation.
    ///
    /// # Errors
    /// [`ShiftError::Money`] on currency mismatch or overflow.
    pub fn between(counted: Money, expected: Money) -> Result<Self, ShiftError> {
        let delta = counted.checked_sub(expected)?;
        Ok(if delta.is_zero() {
            Self::Balanced
        } else if delta.is_negative() {
            Self::Short {
                by: delta.checked_neg()?,
            }
        } else {
            Self::Over { by: delta }
        })
    }

    /// Size of the discrepancy, whichever direction.
    #[must_use]
    pub fn magnitude(self) -> Money {
        match self {
            Self::Balanced => Money::zero(crate::Currency::Bdt),
            Self::Short { by } | Self::Over { by } => by,
        }
    }

    #[must_use]
    pub const fn is_balanced(self) -> bool {
        matches!(self, Self::Balanced)
    }
}

/// What a shift came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftReport {
    pub shift_id: uuid::Uuid,
    pub cashier: uuid::Uuid,
    /// False for an X report — the session is still running.
    pub is_final: bool,

    pub opening_float: Money,
    /// Everything sold in the shift, all tenders.
    pub takings: Money,
    /// Cash actually kept from sales, after change handed back.
    pub cash_from_sales: Money,
    /// Cash in and out outside sales, netted.
    pub net_movements: Money,
    /// What the drawer should hold.
    pub expected_cash: Money,
    /// What was counted, if it has been.
    pub counted_cash: Option<Money>,
    pub variance: Option<Variance>,

    pub sale_count: usize,
    /// Voided lines across the shift — the raw void-rate signal.
    pub void_count: usize,
    /// How many times the drawer was counted. More than one is worth a look.
    pub count_attempts: usize,
}

/// Build the report for a shift, given every sale that might belong to it.
///
/// Sales are filtered by completion time rather than trusted from the caller, so a report cannot be
/// made to look better by handing it a curated list.
///
/// # Errors
/// [`ShiftError`] on currency mismatch or overflow.
pub fn report<'s, I>(shift: &Shift, sales: I) -> Result<ShiftReport, ShiftError>
where
    I: IntoIterator<Item = &'s Sale> + Clone,
{
    let mine: Vec<&Sale> = sales
        .into_iter()
        .filter(|sale| sale.settled_at().is_some_and(|at| shift.covers(at)))
        .collect();

    let currency = shift.currency();

    let takings = Money::try_sum(
        mine.iter().filter_map(|sale| sale.settled_total()),
        currency,
    )?;
    let cash_from_sales = mine.iter().try_fold(Money::zero(currency), |total, sale| {
        Ok::<_, ShiftError>(total.checked_add(sale.net_cash()?)?)
    })?;

    let net_movements = shift.net_movements()?;
    let expected_cash = shift
        .opening_float()
        .checked_add(cash_from_sales)?
        .checked_add(net_movements)?;

    let counted_cash = shift.final_count().map(|count| count.counted);
    let variance = counted_cash
        .map(|counted| Variance::between(counted, expected_cash))
        .transpose()?;

    Ok(ShiftReport {
        shift_id: shift.id(),
        cashier: shift.opened_by(),
        is_final: shift.status() == ShiftStatus::Closed,
        opening_float: shift.opening_float(),
        takings,
        cash_from_sales,
        net_movements,
        expected_cash,
        counted_cash,
        variance,
        sale_count: mine.len(),
        void_count: mine.iter().map(|sale| sale.void_count()).sum(),
        count_attempts: shift.counts().len(),
    })
}
