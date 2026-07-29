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

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;

use sahl_core::Timestamp;
use sahl_core::inventory::{InventoryEvent, IssueReason};
use sahl_core::money::{Currency, Money, Rate, Rounding};
use sahl_core::outlet::{FiscalRegime, OutletEvent, OutletSettings, Profile};
use sahl_core::purchasing::{CloseReason, OrderLine, PurchaseEvent};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{SaleEvent, TenderMethod, VoidReason, Wallet};
use sahl_core::shift::{CashMovementReason, ShiftEvent};
use sahl_core::staff::{Permission, Role, StaffEvent, pin as staff_pin};
use sahl_core::tax::{Discount, PricingMode, TaxClass};
use uuid::Uuid;

use crate::terminal::{Terminal, TerminalError};

pub use view::{
    AuditView, BatchView, LineView, OrderLineView, OrderView, SaleView, ShiftView, StaffView,
    StockView, TaxGroupView, TenderView, VarianceView,
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
            TerminalError::Sale(_)
            | TerminalError::Shift(_)
            | TerminalError::Inventory(_)
            | TerminalError::Outlet(_) => "rejected",
            TerminalError::Directory(_) | TerminalError::Purchase(_) | TerminalError::Fiscal(_) => {
                "rejected"
            }
            TerminalError::UnknownOrder { .. } => "unknown_order",
            TerminalError::NotAuthorized => "not_authorized",
            TerminalError::NoApprover => "no_approver",
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

/// Authenticate an approver and return their id.
///
/// The returned id is what lands in the event's `authorized_by`. A field the UI fills in from a
/// constant records nothing, and every control built on it — the audit feed, the self-approval
/// check — is decorative until this is the only way to produce one.
fn authorize(
    state: &TerminalState,
    permission: Permission,
    pin: &str,
) -> Result<Uuid, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;
    Ok(terminal.approve(permission, pin)?)
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
    // `standard`, `zero_rated`, or `exempt`.
    tax_treatment: String,
    currency: String,
) -> CommandResult {
    let currency = Currency::from_code(&currency).map_err(|error| CommandError {
        code: "bad_currency",
        message: error.to_string(),
    })?;

    let unit_price = Money::from_minor(unit_price_minor, currency);
    let quantity = Quantity::from_milli(quantity_milli);
    let tax_class = tax_class(&tax_treatment, tax_basis_points)?;

    // Tapping the same item twice should read as "two of those", not two identical rows a cashier
    // has to scroll past. Matched on everything that makes a line the same supply — a different
    // price or tax treatment is a different line even for the same product.
    //
    // The addition happens here rather than in the webview because it is arithmetic on a quantity,
    // and quantities are the same kind of exact integer as money.
    let existing = {
        let terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;

        terminal.sale(sale_id)?.active_lines().find_map(|line| {
            let same_supply = line.product_id == product_id
                && line.unit_price == unit_price
                && line.tax_class == tax_class
                // A discounted line keeps its own row: merging would silently spread one line's
                // reduction across units that were never discounted.
                && matches!(line.discount, Discount::None);

            same_supply.then_some((line.id, line.quantity))
        })
    };

    if let Some((line_id, current)) = existing {
        let merged = current
            .checked_add(quantity)
            .map_err(|error| CommandError {
                code: "rejected",
                message: error.to_string(),
            })?;

        return apply(
            &state,
            &SaleEvent::LineQuantityChanged {
                sale_id,
                line_id,
                quantity: merged,
            },
        );
    }

    apply(
        &state,
        &SaleEvent::LineAdded {
            sale_id,
            line_id: new_id(),
            product_id,
            name,
            unit_price,
            quantity,
            tax_class,
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
    // A manager's own PIN, typed at the till. Never an id the UI chose.
    pin: String,
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

    let authorized_by = authorize(&state, Permission::VoidLine, &pin)?;
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
    pin: String,
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

    let authorized_by = authorize(&state, Permission::ApplyDiscount, &pin)?;
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
pub fn complete_sale(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
    cashier_id: Uuid,
) -> CommandResult {
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

    let mut terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    // Read before the mutable borrow. The regime is whatever the outlet is configured as, and
    // "none" while it is unconfigured — a real deployment, not a placeholder.
    let regime = terminal.regime();

    terminal.complete_sale(
        &SaleEvent::Completed {
            sale_id,
            total,
            change_given: change,
            // The completion time attributes the sale to a shift, so it comes from the till's
            // clock at the moment of closing, not from the UI.
            at: now(),
        },
        regime,
        cashier_id,
        now(),
    )?;

    let sale = terminal.sale(sale_id)?;
    SaleView::of(sale).map_err(|error| CommandError {
        code: "rejected",
        message: error.to_string(),
    })
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
    pin: String,
) -> ShiftResult {
    let reason = cash_reason(&reason)?;
    let authorized_by = authorize(&state, Permission::MoveCash, &pin)?;
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

/// Map the UI's tax treatment onto the domain enum.
///
/// Three treatments, not one rate. Standard-at-zero, zero-rated and exempt all charge the customer
/// nothing and are three different things on a VAT return: zero-rated keeps input VAT reclaimable,
/// exempt does not, and a rate of zero on a standard supply is neither. Collapsing them into a rate
/// makes the return wrong in a way no total on any screen would reveal.
fn tax_class(treatment: &str, basis_points: i32) -> Result<TaxClass, CommandError> {
    match treatment {
        "standard" => Ok(TaxClass::standard(basis_points)),
        "zero_rated" => Ok(TaxClass::ZeroRated),
        "exempt" => Ok(TaxClass::Exempt),
        other => Err(CommandError {
            code: "bad_tax_class",
            message: format!("{other} is not a tax treatment"),
        }),
    }
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

// -------------------------------------------------------------------------------------------
// Staff
// -------------------------------------------------------------------------------------------

/// Who can sign in at this till.
#[tauri::command]
pub fn staff_list(state: tauri::State<'_, TerminalState>) -> Result<Vec<StaffView>, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    Ok(terminal
        .staff()
        .active()
        .into_iter()
        .map(StaffView::of)
        .collect())
}

/// Sign one named person in, returning them if the PIN matches.
#[tauri::command]
pub fn sign_in(
    state: tauri::State<'_, TerminalState>,
    staff_id: Uuid,
    pin: String,
) -> Result<StaffView, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    let id = terminal.sign_in(staff_id, &pin)?;
    terminal
        .staff()
        .get(id)
        .map(StaffView::of)
        .ok_or(CommandError {
            code: "not_authorized",
            message: "that PIN was not accepted".to_owned(),
        })
}

/// Enrol a staff member.
///
/// Salting happens here rather than in `sahl-core`, which stays free of randomness so the terminal
/// and server compute identically from identical inputs.
#[tauri::command]
pub fn enrol_staff(
    state: tauri::State<'_, TerminalState>,
    name: String,
    role: String,
    new_pin: String,
    pin: String,
) -> Result<Vec<StaffView>, CommandError> {
    let role = staff_role(&role)?;

    let mut terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    // The first person enrolled cannot be approved by anyone — there is nobody yet. After that,
    // managing staff is an owner's job and the PIN has to prove it.
    let enrolled_by = if terminal.staff().is_empty() {
        // ...and they must be an owner. Only an owner can enrol anyone, so a first cashier would
        // leave the outlet permanently unable to add staff — with no way out short of editing the
        // event log by hand.
        if !matches!(role, Role::Owner) {
            return Err(CommandError {
                code: "first_must_be_owner",
                message: "the first person enrolled must be an owner — nobody else can add staff"
                    .to_owned(),
            });
        }
        Uuid::nil()
    } else {
        terminal.approve(Permission::ManageStaff, &pin)?
    };

    let salt = SaltString::generate(&mut OsRng);
    let pin_hash = staff_pin::hash(&new_pin, &salt).map_err(|error| CommandError {
        code: "bad_pin",
        message: error.to_string(),
    })?;

    terminal.record_staff(
        &StaffEvent::Enrolled {
            staff_id: new_id(),
            name: name.trim().to_owned(),
            role,
            pin_hash,
            at: now(),
            enrolled_by,
        },
        new_id(),
        now(),
    )?;

    Ok(terminal
        .staff()
        .active()
        .into_iter()
        .map(StaffView::of)
        .collect())
}

/// The audit feed: actions that moved money without selling something.
///
/// Names are resolved here rather than in the webview, which has no staff list and should not need
/// one. `unapproved` is the judged signal — self-approval by someone whose role did not carry it.
#[tauri::command]
pub fn audit_feed(state: tauri::State<'_, TerminalState>) -> Result<Vec<AuditView>, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    let entries = sahl_core::staff::ranked(terminal.audit_entries()?);
    let flagged: std::collections::BTreeSet<_> =
        sahl_core::staff::unapproved(&entries, |actor| terminal.staff().role_of(actor))
            .into_iter()
            .map(|entry| (entry.at.millis(), entry.kind, entry.actor))
            .collect();

    let name_of = |id: Uuid| {
        terminal
            .staff()
            .get(id)
            .map_or_else(|| format!("Unknown ({id})"), |member| member.name.clone())
    };

    Ok(entries
        .iter()
        .map(|entry| AuditView {
            at: entry.at.millis(),
            severity: severity_label(entry.severity),
            kind: entry.kind,
            actor: entry.actor,
            actor_name: name_of(entry.actor),
            approved_by: entry.approved_by,
            approved_by_name: entry.approved_by.map(name_of),
            amount_minor: entry.amount.map(Money::minor),
            summary: entry.summary.clone(),
            unapproved: flagged.contains(&(entry.at.millis(), entry.kind, entry.actor)),
        })
        .collect())
}

const fn severity_label(severity: sahl_core::staff::Severity) -> &'static str {
    match severity {
        sahl_core::staff::Severity::Routine => "routine",
        sahl_core::staff::Severity::Notable => "notable",
        sahl_core::staff::Severity::Alert => "alert",
    }
}

/// Map the UI's role string onto the domain enum.
fn staff_role(role: &str) -> Result<Role, CommandError> {
    match role {
        "cashier" => Ok(Role::Cashier),
        "manager" => Ok(Role::Manager),
        "owner" => Ok(Role::Owner),
        other => Err(CommandError {
            code: "bad_role",
            message: format!("{other} is not a role"),
        }),
    }
}

// -------------------------------------------------------------------------------------------
// Purchase orders
// -------------------------------------------------------------------------------------------

type OrderResult = Result<Vec<OrderView>, CommandError>;

fn order_views(terminal: &Terminal) -> OrderResult {
    terminal
        .orders()
        .into_iter()
        .map(|order| {
            OrderView::of(order, Currency::Bdt).map_err(|error| CommandError {
                code: "rejected",
                message: error.to_string(),
            })
        })
        .collect()
}

fn with_orders<F>(state: &TerminalState, act: F) -> OrderResult
where
    F: FnOnce(&mut Terminal) -> Result<(), TerminalError>,
{
    let mut terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    act(&mut terminal)?;
    order_views(&terminal)
}

/// One line of an order as the UI sends it.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderLineInput {
    pub product_id: Uuid,
    pub quantity_milli: i64,
    pub unit_cost_minor: i64,
}

/// Place an order with a supplier.
#[tauri::command]
pub fn place_order(
    state: tauri::State<'_, TerminalState>,
    supplier: String,
    reference: Option<String>,
    expected_at_millis: Option<i64>,
    lines: Vec<OrderLineInput>,
    placed_by: Uuid,
) -> OrderResult {
    if supplier.trim().is_empty() {
        return Err(CommandError {
            code: "bad_supplier",
            message: "an order needs a supplier".to_owned(),
        });
    }

    let lines: Vec<OrderLine> = lines
        .into_iter()
        .map(|line| OrderLine {
            line_id: new_id(),
            product_id: line.product_id,
            quantity: Quantity::from_milli(line.quantity_milli),
            unit_cost: Money::from_minor(line.unit_cost_minor, Currency::Bdt),
        })
        .collect();

    with_orders(&state, |terminal| {
        terminal
            .record_purchase(
                &PurchaseEvent::Placed {
                    order_id: new_id(),
                    supplier: supplier.trim().to_owned(),
                    reference: reference.and_then(|value| {
                        let trimmed = value.trim().to_owned();
                        (!trimmed.is_empty()).then_some(trimmed)
                    }),
                    lines,
                    expected_at: expected_at_millis.map(Timestamp::from_millis),
                    at: now(),
                    placed_by,
                },
                new_id(),
                now(),
            )
            .map(|_| ())
    })
}

/// Book part or all of a line in, creating the batch it becomes.
///
/// Two events, one action: the order records that stock arrived against it, and the inventory book
/// records the batch on the shelf. Recording only one of them is how a delivery ends up either
/// invisible to a recall or invisible to the supplier reconciliation.
#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "a receipt against an order carries this many independent facts; a struct would only \
              move the argument list to the TypeScript side"
)]
pub fn receive_against_order(
    state: tauri::State<'_, TerminalState>,
    order_id: Uuid,
    line_id: Uuid,
    quantity_milli: i64,
    unit_cost_minor: i64,
    lot: Option<String>,
    expires_at_millis: Option<i64>,
    received_by: Uuid,
) -> OrderResult {
    with_orders(&state, |terminal| {
        let product_id = terminal
            .order(order_id)?
            .line(line_id)
            .ok_or(TerminalError::UnknownOrder { order_id })?
            .line
            .product_id;

        let supplier = terminal.order(order_id)?.supplier.clone();
        let batch_id = new_id();
        let unit_cost = Money::from_minor(unit_cost_minor, Currency::Bdt);
        let quantity = Quantity::from_milli(quantity_milli);

        terminal.record_receipt(
            &PurchaseEvent::LineReceived {
                order_id,
                line_id,
                batch_id,
                quantity,
                unit_cost,
                at: now(),
                received_by,
            },
            &InventoryEvent::BatchReceived {
                batch_id,
                product_id,
                lot,
                expires_at: expires_at_millis.map(Timestamp::from_millis),
                quantity,
                unit_cost,
                supplier: Some(supplier),
                at: now(),
                received_by,
            },
            now(),
        )
    })
}

/// Finish with an order, whether or not everything arrived.
#[tauri::command]
pub fn close_order(
    state: tauri::State<'_, TerminalState>,
    order_id: Uuid,
    reason: String,
    closed_by: Uuid,
) -> OrderResult {
    let reason = close_reason(&reason)?;
    with_orders(&state, |terminal| {
        terminal
            .record_purchase(
                &PurchaseEvent::Closed {
                    order_id,
                    reason,
                    at: now(),
                    closed_by,
                },
                new_id(),
                now(),
            )
            .map(|_| ())
    })
}

/// Every order this outlet knows about.
#[tauri::command]
pub fn order_list(state: tauri::State<'_, TerminalState>) -> OrderResult {
    with_orders(&state, |_| Ok(()))
}

/// Map the UI's reason string onto the domain enum.
///
/// Rejected rather than defaulted: "short shipped" and "cancelled" describe different suppliers,
/// and filing one as the other is how a supplier's reliability becomes unmeasurable.
fn close_reason(reason: &str) -> Result<CloseReason, CommandError> {
    match reason {
        "complete" => Ok(CloseReason::Complete),
        "short_shipped" => Ok(CloseReason::ShortShipped),
        "cancelled" => Ok(CloseReason::Cancelled),
        other => Err(CommandError {
            code: "unknown_reason",
            message: format!("{other} is not a reason to close an order"),
        }),
    }
}

// -------------------------------------------------------------------------------------------
// Outlet setup
// -------------------------------------------------------------------------------------------

/// How this outlet trades, as the settings screen shows it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutletView {
    pub outlet_id: Uuid,
    pub name: String,
    pub profile: &'static str,
    pub currency: &'static str,
    pub timezone: String,
    pub regime: &'static str,
    pub tax_registration: Option<String>,
    pub address: String,
    pub configured_at: i64,
    /// What this profile can do, so a screen need not reimplement the table.
    pub capabilities: Vec<&'static str>,
}

/// The outlet's configuration, or `None` if setup has not been done.
#[tauri::command]
pub fn outlet_config(
    state: tauri::State<'_, TerminalState>,
) -> Result<Option<OutletView>, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    Ok(terminal.outlet().map(|outlet| OutletView {
        outlet_id: outlet.outlet_id,
        name: outlet.name.clone(),
        profile: outlet.profile.label(),
        currency: outlet.currency.code(),
        timezone: outlet.timezone.clone(),
        regime: outlet.regime.label(),
        tax_registration: outlet.tax_registration.clone(),
        address: outlet.address.clone(),
        configured_at: outlet.configured_at.millis(),
        capabilities: outlet
            .profile
            .capabilities()
            .into_iter()
            .map(capability_label)
            .collect(),
    }))
}

/// Set the outlet up, or change its settings.
///
/// A full replacement rather than a patch: a patch that arrives out of order leaves an outlet in a
/// state nobody chose, and these events reach a till that may have been offline for a week.
#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "outlet setup genuinely carries this many independent facts, and grouping them into a               struct would only move the argument list to the TypeScript side"
)]
pub fn configure_outlet(
    state: tauri::State<'_, TerminalState>,
    name: String,
    profile: String,
    currency: String,
    timezone: String,
    regime: String,
    tax_registration: Option<String>,
    address: String,
    pin: String,
) -> Result<Option<OutletView>, CommandError> {
    let profile = match profile.as_str() {
        "retail" => Profile::Retail,
        "cafe" => Profile::Cafe,
        "grocery" => Profile::Grocery,
        other => {
            return Err(CommandError {
                code: "bad_profile",
                message: format!("{other} is not a profile"),
            });
        }
    };

    let currency = Currency::from_code(&currency).map_err(|error| CommandError {
        code: "bad_currency",
        message: error.to_string(),
    })?;

    let regime = FiscalRegime::from_label(&regime).map_err(|error| CommandError {
        code: "bad_regime",
        message: error.to_string(),
    })?;

    let outlet_id = {
        let terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;
        terminal.identity().outlet_id
    };

    // Changing the BIN an outlet trades under is not a cashier's decision. The first setup is
    // allowed unapproved for the same reason the first staff enrolment is — nobody exists yet.
    let configured_by = {
        let terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;
        if terminal.staff().is_empty() {
            Uuid::nil()
        } else {
            terminal.approve(Permission::ManageStaff, &pin)?
        }
    };

    let mut terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    terminal.record_outlet(
        &OutletEvent::Configured {
            outlet_id,
            settings: OutletSettings {
                name: name.trim().to_owned(),
                profile,
                currency,
                timezone: timezone.trim().to_owned(),
                regime,
                tax_registration: tax_registration.and_then(|value| {
                    let trimmed = value.trim().to_owned();
                    (!trimmed.is_empty()).then_some(trimmed)
                }),
                address: address.trim().to_owned(),
            },
            at: now(),
            configured_by,
        },
        new_id(),
        now(),
    )?;

    drop(terminal);
    outlet_config(state)
}

const fn capability_label(capability: sahl_core::outlet::Capability) -> &'static str {
    use sahl_core::outlet::Capability as C;
    match capability {
        C::OpenTickets => "open_tickets",
        C::TableService => "table_service",
        C::KitchenRouting => "kitchen_routing",
        C::LineModifiers => "line_modifiers",
        C::CourseFiring => "course_firing",
        C::SplitBills => "split_bills",
        C::WeighedItems => "weighed_items",
        C::ScaleIntegration => "scale_integration",
        C::BatchExpiry => "batch_expiry",
        C::CashDrawer => "cash_drawer",
        // Capability is #[non_exhaustive]; an unnamed one renders honestly rather than as a guess.
        _ => "unknown",
    }
}
