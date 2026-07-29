//! The commands the webview may call.
//!
//! This is the entire API surface between the UI and the till. There is no SQL access, no direct
//! event append, and no arithmetic — which is what makes the money guarantee structural rather than
//! a convention.
//!
//! Every mutating command returns the full [`SaleView`], so the UI never reconstructs state from a
//! delta or re-fetches after an action. A round trip it might skip is a round trip that eventually
//! drifts.

mod view;

use std::sync::Mutex;

use sahl_core::Timestamp;
use sahl_core::money::{Currency, Money, Rate, Rounding};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{SaleEvent, TenderMethod, VoidReason, Wallet};
use sahl_core::tax::{Discount, PricingMode, TaxClass};
use uuid::Uuid;

use crate::terminal::{Terminal, TerminalError};

pub use view::{LineView, SaleView, TaxGroupView, TenderView};

/// Managed Tauri state.
///
/// A `Mutex` rather than anything cleverer: a till serves one customer at a time, commands are
/// microseconds, and a lock that is trivially correct beats a lock-free design nobody can audit
/// when the thing being guarded is a merchant's money.
#[derive(Debug)]
pub struct TerminalState {
    /// Shared with the background sync thread, which takes the lock only for a round.
    inner: std::sync::Arc<Mutex<Terminal>>,
}

impl TerminalState {
    #[must_use]
    pub fn new(terminal: Terminal) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(terminal)),
        }
    }

    /// Wrap a till already shared with the sync thread.
    #[must_use]
    pub const fn from_shared(inner: std::sync::Arc<Mutex<Terminal>>) -> Self {
        Self { inner }
    }
}

/// What the UI receives when something goes wrong.
///
/// A code plus a human message: the code is what the UI branches on, the message is what a cashier
/// reads. Neither is a Rust type name, because "SaleError::NonCashOvertender" is not a sentence
/// anyone should see at a counter.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

impl From<TerminalError> for CommandError {
    fn from(error: TerminalError) -> Self {
        let code = match &error {
            TerminalError::CorruptLog { .. } => "corrupt_log",
            TerminalError::UnknownSale { .. } => "unknown_sale",
            TerminalError::Store(_) => "storage",
            TerminalError::Event(_) => "event",
            TerminalError::Sale(_) => "rejected",
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

type CommandResult = Result<SaleView, CommandError>;

/// Wall-clock time, supplied here rather than inside `sahl-core` so the domain stays a pure
/// function of its inputs and remains replayable.
fn now() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    Timestamp::from_millis(millis)
}

/// UUID v7 — time-sortable, so a log sorted by id is also in creation order.
fn new_id() -> Uuid {
    Uuid::now_v7()
}

fn apply(state: &TerminalState, event: &SaleEvent) -> CommandResult {
    let mut terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    terminal.record(event, new_id(), now())?;
    let sale = terminal.sale(event.sale_id())?;
    SaleView::of(sale).map_err(|error| CommandError {
        code: "rejected",
        message: error.to_string(),
    })
}

#[tauri::command]
pub fn open_sale(state: tauri::State<'_, TerminalState>, cashier_id: Uuid) -> CommandResult {
    apply(
        &state,
        &SaleEvent::Opened {
            sale_id: new_id(),
            opened_by: cashier_id,
            currency: Currency::Bdt,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        },
    )
}

#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "a scanned line genuinely carries this many independent facts, and grouping them into \
              a struct would only move the argument list to the TypeScript side"
)]
pub fn add_line(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
    product_id: Uuid,
    name: String,
    unit_price_minor: i64,
    quantity_milli: i64,
    tax_basis_points: i32,
    currency: String,
) -> CommandResult {
    let currency = Currency::from_code(&currency).map_err(|error| CommandError {
        code: "bad_currency",
        message: error.to_string(),
    })?;

    apply(
        &state,
        &SaleEvent::LineAdded {
            sale_id,
            line_id: new_id(),
            product_id,
            name,
            unit_price: Money::from_minor(unit_price_minor, currency),
            quantity: Quantity::from_milli(quantity_milli),
            tax_class: TaxClass::standard(tax_basis_points),
        },
    )
}

#[tauri::command]
pub fn change_quantity(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
    line_id: Uuid,
    quantity_milli: i64,
) -> CommandResult {
    apply(
        &state,
        &SaleEvent::LineQuantityChanged {
            sale_id,
            line_id,
            quantity: Quantity::from_milli(quantity_milli),
        },
    )
}

#[tauri::command]
pub fn void_line(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
    line_id: Uuid,
    reason: String,
    authorized_by: Uuid,
) -> CommandResult {
    let reason = match reason.as_str() {
        "mistake" => VoidReason::Mistake,
        "customer_changed" => VoidReason::CustomerChanged,
        "damaged" => VoidReason::Damaged,
        "unavailable" => VoidReason::Unavailable,
        other => {
            return Err(CommandError {
                code: "bad_reason",
                // A void without a real reason is a void that tells an owner nothing, so an
                // unrecognised one is refused rather than defaulted.
                message: format!("unknown void reason: {other}"),
            });
        }
    };

    apply(
        &state,
        &SaleEvent::LineVoided {
            sale_id,
            line_id,
            reason,
            authorized_by,
        },
    )
}

#[tauri::command]
pub fn discount_order(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
    amount_minor: Option<i64>,
    basis_points: Option<i32>,
    currency: String,
    authorized_by: Uuid,
) -> CommandResult {
    let currency = Currency::from_code(&currency).map_err(|error| CommandError {
        code: "bad_currency",
        message: error.to_string(),
    })?;

    let discount = match (amount_minor, basis_points) {
        (Some(minor), None) => Discount::Amount {
            amount: Money::from_minor(minor, currency),
        },
        (None, Some(points)) => Discount::Percentage {
            rate: Rate::from_basis_points(points),
        },
        _ => {
            return Err(CommandError {
                code: "bad_discount",
                message: "give exactly one of amount or percentage".to_owned(),
            });
        }
    };

    apply(
        &state,
        &SaleEvent::OrderDiscounted {
            sale_id,
            discount,
            authorized_by,
        },
    )
}

#[tauri::command]
pub fn record_tender(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
    method: String,
    amount_minor: i64,
    currency: String,
    reference: Option<String>,
) -> CommandResult {
    let currency = Currency::from_code(&currency).map_err(|error| CommandError {
        code: "bad_currency",
        message: error.to_string(),
    })?;

    let method = match method.as_str() {
        "cash" => TenderMethod::Cash,
        "card" => TenderMethod::Card,
        "bkash" => TenderMethod::MobileWallet {
            wallet: Wallet::Bkash,
        },
        "nagad" => TenderMethod::MobileWallet {
            wallet: Wallet::Nagad,
        },
        "rocket" => TenderMethod::MobileWallet {
            wallet: Wallet::Rocket,
        },
        "upay" => TenderMethod::MobileWallet {
            wallet: Wallet::Upay,
        },
        "stc_pay" => TenderMethod::MobileWallet {
            wallet: Wallet::StcPay,
        },
        "bank_transfer" => TenderMethod::BankTransfer,
        "store_credit" => TenderMethod::StoreCredit,
        other => {
            return Err(CommandError {
                code: "bad_tender",
                message: format!("unknown tender method: {other}"),
            });
        }
    };

    apply(
        &state,
        &SaleEvent::TenderRecorded {
            sale_id,
            tender_id: new_id(),
            method,
            amount: Money::from_minor(amount_minor, currency),
            reference,
        },
    )
}

/// Close the sale.
///
/// The total and change are taken from the till's own calculation, never from the UI. The webview
/// could not supply them correctly even if it wanted to, which is the point.
#[tauri::command]
pub fn complete_sale(state: tauri::State<'_, TerminalState>, sale_id: Uuid) -> CommandResult {
    let (total, change) = {
        let terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;
        let sale = terminal.sale(sale_id)?;
        let total = sale.totals().map_err(|error| CommandError {
            code: "rejected",
            message: error.to_string(),
        })?;
        let change = sale.change_due().map_err(|error| CommandError {
            code: "rejected",
            message: error.to_string(),
        })?;
        (total.total, change)
    };

    apply(
        &state,
        &SaleEvent::Completed {
            sale_id,
            total,
            change_given: change,
        },
    )
}

#[tauri::command]
pub fn abandon_sale(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
    abandoned_by: Uuid,
) -> CommandResult {
    apply(
        &state,
        &SaleEvent::Abandoned {
            sale_id,
            abandoned_by,
        },
    )
}

#[tauri::command]
pub fn get_sale(state: tauri::State<'_, TerminalState>, sale_id: Uuid) -> CommandResult {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;
    SaleView::of(terminal.sale(sale_id)?).map_err(|error| CommandError {
        code: "rejected",
        message: error.to_string(),
    })
}

/// Shift banner: takings so far, and how much is still waiting to sync.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TillStatus {
    pub takings_minor: i64,
    pub currency: &'static str,
    pub unsynced_count: u64,
    pub open_sales: usize,
}

#[tauri::command]
pub fn till_status(state: tauri::State<'_, TerminalState>) -> Result<TillStatus, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    Ok(TillStatus {
        takings_minor: terminal.takings(Currency::Bdt)?.minor(),
        currency: Currency::Bdt.code(),
        unsynced_count: terminal.unsynced_count()?,
        open_sales: terminal.book().open().count(),
    })
}
