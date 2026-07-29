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
use sahl_core::inventory::{InventoryEvent, IssueReason};
use sahl_core::money::{Currency, Money, Rate, Rounding};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{SaleEvent, TenderMethod, VoidReason, Wallet};
use sahl_core::shift::{CashMovementReason, ShiftEvent};
use sahl_core::tax::{Discount, PricingMode, TaxClass};
use uuid::Uuid;

use crate::terminal::{Terminal, TerminalError};

pub use view::{
    BatchView, LineView, SaleView, ShiftView, StockView, TaxGroupView, TenderView, VarianceView,
};

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
            TerminalError::TicketHeld { .. } => "ticket_held",
            TerminalError::Store(_) => "storage",
            TerminalError::Event(_) => "event",
            TerminalError::Sale(_) | TerminalError::Shift(_) | TerminalError::Inventory(_) => {
                "rejected"
            }
            TerminalError::NoOpenShift => "no_open_shift",
            TerminalError::ShiftAlreadyOpen => "shift_already_open",
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
            // The completion time attributes the sale to a shift, so it comes from the till's
            // clock at the moment of closing, not from the UI.
            at: now(),
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

/// Live sync state for the header badge.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SyncView {
    /// Sync is not configured — a single-till shop that never syncs is a valid deployment.
    Disabled,
    UpToDate {
        unsynced: u64,
    },
    Retrying {
        unsynced: u64,
        attempts: u32,
    },
    /// Needs a person. Kept distinct from `Retrying` so the UI can say so.
    Stopped {
        reason: String,
    },
}

/// Report sync state, tolerating sync not running at all.
///
/// `try_state` rather than `State`: when SAHL_SERVER_URL is unset no handle is managed, and a
/// command that errored in that case would make a perfectly valid offline-only shop look broken.
#[tauri::command]
pub fn sync_status(app: tauri::AppHandle) -> SyncView {
    use tauri::Manager as _;

    let Some(handle) = app.try_state::<crate::sync::SyncHandle>() else {
        return SyncView::Disabled;
    };

    match handle.status() {
        crate::sync::SyncStatus::UpToDate { unsynced } => SyncView::UpToDate { unsynced },
        crate::sync::SyncStatus::Retrying { unsynced, attempts } => {
            SyncView::Retrying { unsynced, attempts }
        }
        crate::sync::SyncStatus::Stopped { reason } => SyncView::Stopped { reason },
    }
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

// -------------------------------------------------------------------------------------------
// Shifts
// -------------------------------------------------------------------------------------------

type ShiftResult = Result<ShiftView, CommandError>;

fn with_shift<F>(state: &TerminalState, act: F) -> ShiftResult
where
    F: FnOnce(&mut Terminal) -> Result<(), TerminalError>,
{
    let mut terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    act(&mut terminal)?;
    Ok(ShiftView::of(&terminal.shift_report()?, Currency::Bdt))
}

/// Take the till, counting in the starting float.
#[tauri::command]
pub fn open_shift(
    state: tauri::State<'_, TerminalState>,
    cashier_id: Uuid,
    opening_float_minor: i64,
) -> ShiftResult {
    with_shift(&state, |terminal| {
        terminal
            .record_shift(
                &ShiftEvent::Opened {
                    shift_id: new_id(),
                    currency: Currency::Bdt,
                    opened_by: cashier_id,
                    opening_float: Money::from_minor(opening_float_minor, Currency::Bdt),
                    at: now(),
                },
                new_id(),
                now(),
            )
            .map(|_| ())
    })
}

/// Move cash in or out of the drawer outside a sale.
///
/// `authorized_by` is required rather than optional. Every path that takes money out of a till
/// without a sale names someone, at the moment it happens — reconstructing it afterwards from who
/// was rostered is exactly the reconstruction that never holds up.
#[tauri::command]
pub fn move_cash(
    state: tauri::State<'_, TerminalState>,
    amount_minor: i64,
    reason: String,
    note: Option<String>,
    authorized_by: Uuid,
) -> ShiftResult {
    let reason = cash_reason(&reason)?;
    with_shift(&state, |terminal| {
        let shift_id = terminal.shift().ok_or(TerminalError::NoOpenShift)?.id();
        terminal
            .record_shift(
                &ShiftEvent::CashMoved {
                    shift_id,
                    movement_id: new_id(),
                    amount: Money::from_minor(amount_minor, Currency::Bdt),
                    reason,
                    note,
                    authorized_by,
                    at: now(),
                },
                new_id(),
                now(),
            )
            .map(|_| ())
    })
}

/// Record a physical count of the drawer.
#[tauri::command]
pub fn count_drawer(
    state: tauri::State<'_, TerminalState>,
    counted_minor: i64,
    counted_by: Uuid,
) -> ShiftResult {
    with_shift(&state, |terminal| {
        let shift_id = terminal.shift().ok_or(TerminalError::NoOpenShift)?.id();
        terminal
            .record_shift(
                &ShiftEvent::Counted {
                    shift_id,
                    counted: Money::from_minor(counted_minor, Currency::Bdt),
                    counted_by,
                    at: now(),
                },
                new_id(),
                now(),
            )
            .map(|_| ())
    })
}

/// The X report: where the shift stands, without ending it.
#[tauri::command]
pub fn shift_report(state: tauri::State<'_, TerminalState>) -> ShiftResult {
    with_shift(&state, |_| Ok(()))
}

/// The same figures with every expectation withheld, for a blind count.
///
/// A separate command rather than a flag on [`shift_report`]: a screen that receives the expected
/// figure can leak it, and the safest way to not leak a number is to not send it.
#[tauri::command]
pub fn blind_count_sheet(state: tauri::State<'_, TerminalState>) -> ShiftResult {
    with_shift(&state, |_| Ok(())).map(ShiftView::blind)
}

/// Close the till. Nothing may be added to the shift afterwards.
#[tauri::command]
pub fn close_shift(
    state: tauri::State<'_, TerminalState>,
    closed_by: Uuid,
    closing_cash_minor: i64,
) -> ShiftResult {
    with_shift(&state, |terminal| {
        let shift_id = terminal.shift().ok_or(TerminalError::NoOpenShift)?.id();
        terminal
            .record_shift(
                &ShiftEvent::Closed {
                    shift_id,
                    closed_by,
                    closing_cash: Money::from_minor(closing_cash_minor, Currency::Bdt),
                    at: now(),
                },
                new_id(),
                now(),
            )
            .map(|_| ())
    })
}

/// Map the UI's reason string onto the domain enum.
///
/// Rejected rather than defaulted. A movement filed under the wrong reason is worse than one the
/// till refused, because it looks settled.
fn cash_reason(reason: &str) -> Result<CashMovementReason, CommandError> {
    match reason {
        "float_top_up" => Ok(CashMovementReason::FloatTopUp),
        "skim" => Ok(CashMovementReason::Skim),
        "petty_cash" => Ok(CashMovementReason::PettyCash),
        "refund" => Ok(CashMovementReason::Refund),
        "correction" => Ok(CashMovementReason::Correction),
        other => Err(CommandError {
            code: "unknown_reason",
            message: format!("{other} is not a cash movement reason"),
        }),
    }
}

// -------------------------------------------------------------------------------------------
// Stock
// -------------------------------------------------------------------------------------------

type StockResult = Result<StockView, CommandError>;

fn with_stock<F>(state: &TerminalState, act: F) -> StockResult
where
    F: FnOnce(&mut Terminal) -> Result<(), TerminalError>,
{
    let mut terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    act(&mut terminal)?;
    Ok(StockView::of(terminal.stock(), Currency::Bdt))
}

/// Book a delivery in as a new batch.
///
/// Receiving creates a batch rather than adding to one: a second delivery of the same product is a
/// different lot with its own expiry, and merging them is what makes a recall under-report.
#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "a delivery line carries this many independent facts; a struct would only move the \
              argument list to the TypeScript side"
)]
pub fn receive_stock(
    state: tauri::State<'_, TerminalState>,
    product_id: Uuid,
    lot: Option<String>,
    expires_at_millis: Option<i64>,
    quantity_milli: i64,
    unit_cost_minor: i64,
    supplier: Option<String>,
    received_by: Uuid,
) -> StockResult {
    with_stock(&state, |terminal| {
        terminal
            .record_stock(
                &InventoryEvent::BatchReceived {
                    batch_id: new_id(),
                    product_id,
                    lot,
                    expires_at: expires_at_millis.map(Timestamp::from_millis),
                    quantity: Quantity::from_milli(quantity_milli),
                    unit_cost: Money::from_minor(unit_cost_minor, Currency::Bdt),
                    supplier,
                    at: now(),
                    received_by,
                },
                new_id(),
                now(),
            )
            .map(|_| ())
    })
}

/// Record a physical count of one batch.
///
/// Absolute, not a delta — a count is "there are seven here", and the adjustment is derived. A
/// delta would put the subtraction in a person's hands, which is both a place to err and a place
/// to hide.
#[tauri::command]
pub fn count_stock(
    state: tauri::State<'_, TerminalState>,
    batch_id: Uuid,
    counted_milli: i64,
    counted_by: Uuid,
) -> StockResult {
    with_stock(&state, |terminal| {
        terminal
            .record_stock(
                &InventoryEvent::BatchCounted {
                    batch_id,
                    counted: Quantity::from_milli(counted_milli),
                    at: now(),
                    counted_by,
                },
                new_id(),
                now(),
            )
            .map(|_| ())
    })
}

/// Write stock off, or send it out.
#[tauri::command]
pub fn issue_stock(
    state: tauri::State<'_, TerminalState>,
    batch_id: Uuid,
    quantity_milli: i64,
    reason: String,
    issued_by: Uuid,
) -> StockResult {
    let reason = issue_reason(&reason)?;
    with_stock(&state, |terminal| {
        terminal
            .record_stock(
                &InventoryEvent::StockIssued {
                    batch_id,
                    quantity: Quantity::from_milli(quantity_milli),
                    reason,
                    sale_id: None,
                    at: now(),
                    issued_by,
                },
                new_id(),
                now(),
            )
            .map(|_| ())
    })
}

/// The current stock position.
#[tauri::command]
pub fn stock_position(state: tauri::State<'_, TerminalState>) -> StockResult {
    with_stock(&state, |_| Ok(()))
}

/// The same batches with recorded levels withheld, for a blind count.
#[tauri::command]
pub fn blind_stock_sheet(state: tauri::State<'_, TerminalState>) -> StockResult {
    with_stock(&state, |_| Ok(())).map(StockView::blind)
}

/// Map the UI's reason string onto the domain enum.
///
/// Rejected rather than defaulted: stock written off under the wrong reason is the difference
/// between spoilage the owner can act on and a number nobody can explain.
fn issue_reason(reason: &str) -> Result<IssueReason, CommandError> {
    match reason {
        "wastage" => Ok(IssueReason::Wastage),
        "transfer_out" => Ok(IssueReason::TransferOut),
        "return_to_supplier" => Ok(IssueReason::ReturnToSupplier),
        "internal" => Ok(IssueReason::Internal),
        other => Err(CommandError {
            code: "unknown_reason",
            message: format!("{other} is not a stock issue reason"),
        }),
    }
}
