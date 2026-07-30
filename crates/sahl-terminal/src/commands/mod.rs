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
use sahl_core::catalogue::{CatalogueEvent, ModifierGroup, ProductDetails, Unit};
use sahl_core::floor::{FloorEvent, TableDetails};
use sahl_core::inventory::{InventoryEvent, IssueReason};
use sahl_core::kitchen::Station;
use sahl_core::money::{Currency, Money, Rate, Rounding};
use sahl_core::outlet::{FiscalRegime, OutletEvent, OutletSettings, Profile};
use sahl_core::purchasing::{CloseReason, OrderLine, PurchaseEvent};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{SaleEvent, TenderMethod, VoidReason, Wallet};
use sahl_core::scale::{Embedded, ScaleFormat};
use sahl_core::shift::{CashMovementReason, ShiftEvent};
use sahl_core::staff::{Permission, Role, StaffEvent, pin as staff_pin};
use sahl_core::tax::{Discount, PricingMode, TaxClass};
use sahl_escpos::{
    Document as EscposDocument, KitchenTicketData as EscposKitchenTicket,
    KitchenTicketLine as EscposKitchenLine, PaperWidth,
};
use uuid::Uuid;

use crate::printer::PrinterTarget;
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
            | TerminalError::Outlet(_)
            | TerminalError::FiscalDocument(_)
            | TerminalError::Catalogue(_)
            | TerminalError::Floor(_) => "rejected",
            // Its own code: a corrupt scale label means "scan it again", which is different advice
            // from anything else the till refuses.
            TerminalError::Scale(_) => "bad_scan",
            TerminalError::Weigh(_) => "rejected",
            TerminalError::NotInvoiced { .. } => "not_invoiced",
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
    // Option ids chosen at the till. Validated against the product's groups by the terminal.
    chosen_options: Vec<Uuid>,
    currency: String,
) -> CommandResult {
    let currency = Currency::from_code(&currency).map_err(|error| CommandError {
        code: "bad_currency",
        message: error.to_string(),
    })?;

    let unit_price = Money::from_minor(unit_price_minor, currency);
    let quantity = Quantity::from_milli(quantity_milli);
    let tax_class = tax_class(&tax_treatment, tax_basis_points)?;

    let modifiers = {
        let terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;
        terminal.resolve_modifiers(product_id, &chosen_options)?
    };

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
                // Options make two lines different supplies even for one product. A latte with an
                // extra shot and a latte without are two drinks the kitchen has to make separately.
                && line.modifiers == modifiers
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
            modifiers: Vec::new(),
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
    /// Absent where no scale prints labels, which is every outlet but a grocery.
    pub scale: Option<ScaleFormatView>,
}

/// A configured scale layout, as the settings screen redraws it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScaleFormatView {
    pub prefix: String,
    pub item_digits: u8,
    pub embedded: &'static str,
    pub value_digits: u8,
    pub value_decimals: u8,
    pub filler_digits: u8,
}

impl ScaleFormatView {
    fn of(format: &ScaleFormat) -> Self {
        Self {
            prefix: format.prefix().to_owned(),
            item_digits: format.item_digits(),
            embedded: match format.embedded() {
                Embedded::Weight => "weight",
                Embedded::Price => "price",
            },
            value_digits: format.value_digits(),
            value_decimals: format.value_decimals(),
            filler_digits: format.filler_digits(),
        }
    }
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
        scale: outlet.scale.as_ref().map(ScaleFormatView::of),
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
    scale: Option<ScaleFormatInput>,
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
                scale: scale
                    .map(ScaleFormatInput::into_format)
                    .transpose()
                    .map_err(TerminalError::from)?,
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

// -------------------------------------------------------------------------------------------
// Fiscal documents
// -------------------------------------------------------------------------------------------

/// One row of a Mushak 6.3, by the form's own column numbers.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallanLineView {
    pub serial: u32,
    pub description: String,
    pub unit: String,
    pub quantity_milli: i64,
    /// Column 5 — unit value, excluding tax.
    pub unit_value_minor: i64,
    /// Column 6 — total value, excluding tax.
    pub total_value_minor: i64,
    pub supplementary_duty_minor: i64,
    pub vat_rate_basis_points: i32,
    pub vat_amount_minor: i64,
    pub total_with_tax_minor: i64,
}

/// A fiscal document, or the fact that this outlet owes none.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "regime", rename_all = "snake_case")]
pub enum DocumentView {
    BdMushak63 {
        seller_name: String,
        seller_bin: String,
        issuing_address: String,
        buyer_name: Option<String>,
        buyer_bin: Option<String>,
        invoice_number: String,
        issued_at_millis: i64,
        lines: Vec<ChallanLineView>,
        total_value_minor: i64,
        total_vat_minor: i64,
        total_with_tax_minor: i64,
    },
    /// No regime configured. An ordinary receipt is the whole obligation.
    None,
}

/// The fiscal document for a completed sale.
///
/// Rebuilt on demand rather than stored — see `Terminal::fiscal_document`.
#[tauri::command]
pub fn fiscal_document(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
) -> Result<DocumentView, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    Ok(match terminal.fiscal_document(sale_id)? {
        sahl_fiscal::Document::BdMushak63(challan) => DocumentView::BdMushak63 {
            seller_name: challan.seller_name.clone(),
            seller_bin: challan.seller_bin.clone(),
            issuing_address: challan.issuing_address.clone(),
            buyer_name: challan.buyer_name.clone(),
            buyer_bin: challan.buyer_bin.clone(),
            invoice_number: challan.invoice_number.clone(),
            issued_at_millis: challan.issued_at_millis,
            lines: challan
                .lines
                .iter()
                .map(|line| ChallanLineView {
                    serial: line.serial,
                    description: line.description.clone(),
                    unit: line.unit.clone(),
                    quantity_milli: line.quantity_milli,
                    unit_value_minor: line.unit_value.minor(),
                    total_value_minor: line.total_value.minor(),
                    supplementary_duty_minor: line.supplementary_duty.minor(),
                    vat_rate_basis_points: line.vat_rate_basis_points,
                    vat_amount_minor: line.vat_amount.minor(),
                    total_with_tax_minor: line.total_with_tax.minor(),
                })
                .collect(),
            total_value_minor: challan.total_value.minor(),
            total_vat_minor: challan.total_vat.minor(),
            total_with_tax_minor: challan.total_with_tax.minor(),
        },
        _ => DocumentView::None,
    })
}

// -------------------------------------------------------------------------------------------
// Printing
// -------------------------------------------------------------------------------------------

/// What happened when a receipt was sent to a printer.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintOutcome {
    pub printed: bool,
    /// Why not, when it did not. Shown to the cashier; never a reason to undo a sale.
    pub reason: Option<String>,
    /// How many bytes the job was — useful when diagnosing a printer that accepts and prints
    /// nothing, which is otherwise indistinguishable from success.
    pub bytes: usize,
}

/// Print the receipt for a completed sale.
///
/// A failure here is reported, never propagated into the sale. The money is already in the drawer
/// and the sale is already in the log; paper is a courtesy and a reprintable artefact.
#[tauri::command]
pub fn print_receipt(
    state: tauri::State<'_, TerminalState>,
    printer: tauri::State<'_, PrinterTarget>,
    sale_id: Uuid,
    // Pre-formatted by the UI with `Intl` in the outlet's timezone.
    printed_at: String,
    // `mm58` or `mm80`.
    paper: String,
    open_drawer: bool,
) -> Result<PrintOutcome, CommandError> {
    let paper = match paper.as_str() {
        "mm58" => PaperWidth::Mm58,
        "mm80" => PaperWidth::Mm80,
        other => {
            return Err(CommandError {
                code: "bad_paper",
                // Printing 80mm content on a 58mm roll silently truncates every line, so a wrong
                // width is refused rather than guessed at.
                message: format!("{other} is not a paper width"),
            });
        }
    };

    let data = {
        let terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;
        terminal.receipt(sale_id, printed_at)?
    };

    let job = EscposDocument::render(&data, paper, open_drawer);
    let bytes = job.bytes().len();

    Ok(match crate::printer::print(&printer, job.bytes()) {
        Ok(()) => PrintOutcome {
            printed: true,
            reason: None,
            bytes,
        },
        Err(error) => PrintOutcome {
            printed: false,
            reason: Some(error.to_string()),
            bytes,
        },
    })
}

/// Whether this till has a printer configured at all.
#[tauri::command]
pub fn printer_configured(printer: tauri::State<'_, PrinterTarget>) -> bool {
    printer.is_configured()
}

// -------------------------------------------------------------------------------------------
// Catalogue
// -------------------------------------------------------------------------------------------

/// One choice within a group, as a screen shows it.
///
/// A view rather than the core type: every amount that crosses this boundary is a plain integer of
/// minor units, and `Money` serialises as an object. Leaking it here would make this one field the
/// only place the UI has to know a second money shape.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifierOptionView {
    pub id: Uuid,
    pub name: String,
    /// What choosing it adds to one unit. Zero and negative are both real.
    pub price_delta_minor: i64,
}

/// A set of choices offered on a product.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifierGroupView {
    pub id: Uuid,
    pub name: String,
    pub min: u8,
    pub max: u8,
    pub options: Vec<ModifierOptionView>,
}

impl ModifierGroupView {
    fn of(group: &ModifierGroup) -> Self {
        Self {
            id: group.id,
            name: group.name.clone(),
            min: group.min,
            max: group.max,
            options: group
                .options
                .iter()
                .map(|option| ModifierOptionView {
                    id: option.id,
                    name: option.name.clone(),
                    price_delta_minor: option.price_delta.minor(),
                })
                .collect(),
        }
    }
}

/// A product as the sell screen and the catalogue screen show it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductView {
    pub id: Uuid,
    pub name: String,
    pub sku: Option<String>,
    pub barcodes: Vec<String>,
    pub price_minor: i64,
    /// `pcs`, `kg`, `L` — printed on the receipt and in the Mushak "Unit of Supply" column.
    pub unit: &'static str,
    /// Whether the unit can be sold in fractions, so the UI knows to offer a scale or reject "0.5".
    pub divisible: bool,
    pub tax_basis_points: i32,
    /// `standard`, `zero_rated` or `exempt` — three treatments, not one rate.
    pub tax_treatment: &'static str,
    pub category: Option<String>,
    pub active: bool,
    /// Where this is made, for a café.
    pub station: Option<&'static str>,
    /// Choices offered when this is rung, so the sell screen can draw the chooser.
    pub option_groups: Vec<ModifierGroupView>,
}

impl ProductView {
    fn of(product: &sahl_core::catalogue::Product) -> Self {
        let (tax_treatment, tax_basis_points) = match product.tax_class {
            TaxClass::Standard { rate } => ("standard", rate.basis_points()),
            TaxClass::ZeroRated => ("zero_rated", 0),
            TaxClass::Exempt => ("exempt", 0),
        };

        Self {
            id: product.id,
            name: product.name.clone(),
            sku: product.sku.clone(),
            barcodes: product.barcodes.clone(),
            price_minor: product.price.minor(),
            unit: product.unit.label(),
            divisible: product.unit.is_divisible(),
            tax_basis_points,
            tax_treatment,
            category: product.category.clone(),
            active: product.active,
            station: product.station.map(Station::label),
            option_groups: product
                .option_groups
                .iter()
                .map(ModifierGroupView::of)
                .collect(),
        }
    }
}

/// What the sell screen shows: active products, by name.
#[tauri::command]
pub fn sellable_products(
    state: tauri::State<'_, TerminalState>,
) -> Result<Vec<ProductView>, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    Ok(terminal
        .catalogue()
        .sellable()
        .into_iter()
        .map(ProductView::of)
        .collect())
}

/// The whole catalogue, withdrawn products included.
#[tauri::command]
pub fn all_products(
    state: tauri::State<'_, TerminalState>,
) -> Result<Vec<ProductView>, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    Ok(terminal
        .catalogue()
        .all()
        .into_iter()
        .map(ProductView::of)
        .collect())
}

/// How a counter scale lays out its printed labels, as the settings screen sends it.
///
/// A separate input type because [`ScaleFormat`] validates on construction, and the place to fail
/// is an owner looking at a settings screen — not a cashier mid-queue.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScaleFormatInput {
    pub prefix: String,
    pub item_digits: u8,
    /// `weight` or `price`.
    pub embedded: Embedded,
    pub value_digits: u8,
    pub value_decimals: u8,
    pub filler_digits: u8,
}

impl ScaleFormatInput {
    fn into_format(self) -> Result<ScaleFormat, sahl_core::scale::ScaleError> {
        ScaleFormat::new(
            self.prefix.trim(),
            self.item_digits,
            self.embedded,
            self.value_digits,
            self.value_decimals,
            self.filler_digits,
        )
    }
}

/// What a scan resolved to.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanView {
    pub product: ProductView,
    /// Thousandths. A weighed label brings its own; anything else is one.
    pub quantity_milli: i64,
    /// Set only where the scale already fixed the money. **Sell at this** — repricing from the
    /// catalogue would disagree with the sticker in the customer's hand.
    pub price_minor: Option<i64>,
}

/// Resolve a scanned barcode, unwrapping a scale label where the outlet prints them.
///
/// Returns `None` rather than erroring on an unknown code: an unrecognised scan is an ordinary
/// event at a counter — a loyalty card, a coupon, a competitor's packaging — not a fault. A label
/// that *is* ours and is corrupt does error, because there the cashier needs to scan again rather
/// than go looking on the shelf.
#[tauri::command]
pub fn scan(
    state: tauri::State<'_, TerminalState>,
    barcode: String,
) -> Result<Option<ScanView>, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    let Some(scanned) = terminal.scan(&barcode)? else {
        return Ok(None);
    };
    let Some(product) = terminal.catalogue().get(scanned.product_id) else {
        return Ok(None);
    };

    Ok(Some(ScanView {
        product: ProductView::of(product),
        quantity_milli: scanned.quantity.milli(),
        price_minor: scanned.price.map(sahl_core::Money::minor),
    }))
}

/// Add a product, or change one.
///
/// `product_id` absent means a new product. Editing needs an existing one, and a full replacement
/// rather than a patch — two devices editing while apart cannot have patches merged into a state
/// either intended.
#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "a product genuinely carries this many independent facts, and a struct would only \
              move the argument list to the TypeScript side"
)]
pub fn save_product(
    state: tauri::State<'_, TerminalState>,
    product_id: Option<Uuid>,
    name: String,
    sku: Option<String>,
    barcodes: Vec<String>,
    price_minor: i64,
    unit: String,
    tax_basis_points: i32,
    tax_treatment: String,
    category: Option<String>,
    // Where this is made, for a café. Absent means it needs no preparation.
    station: Option<String>,
    // Choices this product offers. Validated by the catalogue before it is written.
    option_groups: Vec<ModifierGroupInput>,
    pin: String,
) -> Result<Vec<ProductView>, CommandError> {
    let unit = Unit::from_label(&unit).map_err(|_| CommandError {
        code: "bad_unit",
        message: format!("{unit} is not a unit of supply"),
    })?;

    let details = ProductDetails {
        name: name.trim().to_owned(),
        sku: sku.and_then(non_empty),
        barcodes: barcodes.into_iter().filter_map(non_empty).collect(),
        price: Money::from_minor(price_minor, Currency::Bdt),
        unit,
        tax_class: tax_class(&tax_treatment, tax_basis_points)?,
        category: category.and_then(non_empty),
        station: match station
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(label) => Some(Station::from_label(label).map_err(|unknown| CommandError {
                // Never defaulted. An item silently routed to the kitchen instead of the bar is a
                // drink nobody pours, with nothing on any screen to say so.
                code: "bad_station",
                message: format!("{unknown} is not a prep station"),
            })?),
            None => None,
        },
        option_groups: option_groups
            .into_iter()
            .map(|group| ModifierGroup {
                // A new group or option gets an id here rather than from the UI: an id minted by a
                // screen is one two screens can collide on.
                id: group.id.unwrap_or_else(new_id),
                name: group.name.trim().to_owned(),
                min: group.min,
                max: group.max,
                options: group
                    .options
                    .into_iter()
                    .map(|option| sahl_core::catalogue::ModifierOption {
                        id: option.id.unwrap_or_else(new_id),
                        name: option.name.trim().to_owned(),
                        price_delta: Money::from_minor(option.price_delta_minor, Currency::Bdt),
                    })
                    .collect(),
            })
            .collect(),
    };

    let authorized_by = authorize(&state, Permission::EditCatalogue, &pin)?;

    let mut terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    let event = match product_id {
        Some(product_id) => CatalogueEvent::ProductUpdated {
            product_id,
            details,
            at: now(),
            updated_by: authorized_by,
        },
        None => CatalogueEvent::ProductAdded {
            product_id: new_id(),
            details,
            at: now(),
            added_by: authorized_by,
        },
    };

    terminal.record_catalogue(&event, new_id(), now())?;

    Ok(terminal
        .catalogue()
        .all()
        .into_iter()
        .map(ProductView::of)
        .collect())
}

/// Take a product off the sell screen, or put it back.
#[tauri::command]
pub fn set_product_active(
    state: tauri::State<'_, TerminalState>,
    product_id: Uuid,
    active: bool,
    pin: String,
) -> Result<Vec<ProductView>, CommandError> {
    let authorized_by = authorize(&state, Permission::EditCatalogue, &pin)?;

    let mut terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    let event = if active {
        CatalogueEvent::ProductRestored {
            product_id,
            at: now(),
            restored_by: authorized_by,
        }
    } else {
        CatalogueEvent::ProductWithdrawn {
            product_id,
            at: now(),
            withdrawn_by: authorized_by,
        }
    };

    terminal.record_catalogue(&event, new_id(), now())?;

    Ok(terminal
        .catalogue()
        .all()
        .into_iter()
        .map(ProductView::of)
        .collect())
}

/// A group as the catalogue screen sends it. Ids are absent for anything newly added.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifierGroupInput {
    pub id: Option<Uuid>,
    pub name: String,
    pub min: u8,
    pub max: u8,
    pub options: Vec<ModifierOptionInput>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifierOptionInput {
    pub id: Option<Uuid>,
    pub name: String,
    pub price_delta_minor: i64,
}

/// Trim, and treat an empty string as absent.
///
/// A form field someone tabbed through sends `""`, which is not the same as a product having no
/// SKU — and storing one would make an empty string searchable.
fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

// -------------------------------------------------------------------------------------------
// The floor — café only
// -------------------------------------------------------------------------------------------

/// A table as the floor plan shows it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableView {
    pub id: Uuid,
    pub label: String,
    pub section: Option<String>,
    pub seats: u32,
    pub active: bool,
    /// The open ticket sitting here, if any. Derived from the sales, never stored on the table.
    pub sale_id: Option<Uuid>,
    /// What that ticket has run up so far, so a waiter can read the room at a glance.
    pub running_total_minor: Option<i64>,
    pub covers: Option<u32>,
}

/// The floor plan, with each table's current ticket.
#[tauri::command]
pub fn floor_plan(
    state: tauri::State<'_, TerminalState>,
    include_removed: bool,
) -> Result<Vec<TableView>, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    let occupied = terminal.occupied_tables();
    let tables = if include_removed {
        terminal.floor().all()
    } else {
        terminal.floor().in_service()
    };

    Ok(tables
        .into_iter()
        .map(|table| {
            let sale_id = occupied.get(&table.id).copied();
            // Read off the sale rather than recomputed here — the running total is money, and this
            // side of the boundary never computes money.
            let sale = sale_id.and_then(|id| terminal.book().get(id));

            TableView {
                id: table.id,
                label: table.label.clone(),
                section: table.section.clone(),
                seats: table.seats,
                active: table.active,
                sale_id,
                running_total_minor: sale
                    .and_then(|sale| sale.totals().ok())
                    .map(|totals| totals.total.minor()),
                covers: sale.and_then(|sale| sale.seating()).map(|seat| seat.covers),
            }
        })
        .collect())
}

/// Add a table, or change one. `table_id` absent means a new table.
#[tauri::command]
pub fn save_table(
    state: tauri::State<'_, TerminalState>,
    table_id: Option<Uuid>,
    label: String,
    section: Option<String>,
    seats: u32,
    pin: String,
) -> Result<Vec<TableView>, CommandError> {
    let details = TableDetails {
        label: label.trim().to_owned(),
        section: section.and_then(non_empty),
        seats,
    };

    // Changing the floor is a manager's job, not a waiter's: relabelling a table mid-service
    // detaches every open ticket from the room the staff can see.
    let authorized_by = authorize(&state, Permission::EditCatalogue, &pin)?;

    {
        let mut terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;

        let event = match table_id {
            Some(table_id) => FloorEvent::TableUpdated {
                table_id,
                details,
                at: now(),
                updated_by: authorized_by,
            },
            None => FloorEvent::TableAdded {
                table_id: new_id(),
                details,
                at: now(),
                added_by: authorized_by,
            },
        };

        terminal.record_floor(&event, new_id(), now())?;
    }

    floor_plan(state, true)
}

/// Take a table out of service, or put it back.
#[tauri::command]
pub fn set_table_active(
    state: tauri::State<'_, TerminalState>,
    table_id: Uuid,
    active: bool,
    pin: String,
) -> Result<Vec<TableView>, CommandError> {
    let authorized_by = authorize(&state, Permission::EditCatalogue, &pin)?;

    {
        let mut terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;

        // A table with an open ticket on it cannot leave service. The ticket would still exist,
        // sitting at furniture the floor plan no longer shows, and nobody could find it to settle.
        if !active && terminal.occupied_tables().contains_key(&table_id) {
            return Err(CommandError {
                code: "table_occupied",
                message: "settle or move the ticket on this table first".to_owned(),
            });
        }

        let event = if active {
            FloorEvent::TableRestored {
                table_id,
                at: now(),
                restored_by: authorized_by,
            }
        } else {
            FloorEvent::TableRemoved {
                table_id,
                at: now(),
                removed_by: authorized_by,
            }
        };

        terminal.record_floor(&event, new_id(), now())?;
    }

    floor_plan(state, true)
}

/// Seat a ticket at a table, or move it to another one.
#[tauri::command]
pub fn seat_sale(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
    table_id: Uuid,
    covers: u32,
    seated_by: Uuid,
) -> CommandResult {
    {
        let terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;

        if terminal.floor().get(table_id).is_none() {
            return Err(CommandError {
                code: "unknown_table",
                message: "no such table".to_owned(),
            });
        }

        // Two parties on one table is a bill nobody can split correctly afterwards, so it is
        // refused at the point of seating rather than discovered at payment.
        if let Some(sitting) = terminal.occupied_tables().get(&table_id)
            && *sitting != sale_id
        {
            return Err(CommandError {
                code: "table_occupied",
                message: "another ticket is already on that table".to_owned(),
            });
        }
    }

    apply(
        &state,
        &SaleEvent::Seated {
            sale_id,
            table_id,
            covers,
            at: now(),
            seated_by,
        },
    )
}

// -------------------------------------------------------------------------------------------
// Open tickets
// -------------------------------------------------------------------------------------------

/// An open ticket, as the ticket list shows it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketView {
    pub sale_id: Uuid,
    pub line_count: usize,
    /// `None` for a ticket with nothing on it yet.
    pub total_minor: Option<i64>,
    /// The table it is sitting at, for a café.
    pub table_label: Option<String>,
    pub covers: Option<u32>,
    /// Whether another device is holding it. A held ticket cannot be written to from here.
    pub held_elsewhere: bool,
}

/// Every ticket still open on this outlet.
///
/// Without this, a ticket a cashier navigated away from is unreachable: it stays open forever,
/// holding items nobody can settle or clear. That is a slow leak in retail and an outright missing
/// feature in a café, where an open ticket *is* the model.
#[tauri::command]
pub fn open_tickets(
    state: tauri::State<'_, TerminalState>,
) -> Result<Vec<TicketView>, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    let device = terminal.identity().device_id;
    let now = now();

    let mut tickets: Vec<TicketView> = terminal
        .book()
        .open()
        .map(|sale| {
            let seating = sale.seating();
            TicketView {
                sale_id: sale.id(),
                line_count: sale.active_lines().count(),
                total_minor: sale.totals().ok().map(|totals| totals.total.minor()),
                table_label: seating.and_then(|seat| {
                    terminal
                        .floor()
                        .get(seat.table_id)
                        .map(|table| table.label.clone())
                }),
                covers: seating.map(|seat| seat.covers),
                held_elsewhere: matches!(
                    sale.may_write(device, now),
                    sahl_core::policy::lease::ClaimVerdict::Held { .. }
                ),
            }
        })
        .collect();

    // Fullest first. A ticket with items is a customer waiting; an empty one is debris.
    tickets.sort_by(|a, b| {
        b.total_minor
            .unwrap_or_default()
            .cmp(&a.total_minor.unwrap_or_default())
            .then(a.sale_id.cmp(&b.sale_id))
    });
    Ok(tickets)
}

/// Abandon every open ticket that has nothing on it.
///
/// Empty tickets are debris rather than transactions — nobody rang anything, so there is nothing to
/// audit and no signal to preserve. Tickets *with* lines are never touched here: an abandoned basket
/// full of scanned goods is itself something an owner should see, so it has to be abandoned
/// deliberately and attributed to someone.
#[tauri::command]
pub fn discard_empty_tickets(
    state: tauri::State<'_, TerminalState>,
    abandoned_by: Uuid,
) -> Result<usize, CommandError> {
    let empty: Vec<Uuid> = {
        let terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;

        terminal
            .book()
            .open()
            .filter(|sale| sale.active_lines().count() == 0)
            .map(sahl_core::sale::Sale::id)
            .collect()
    };

    let mut discarded = 0_usize;
    for sale_id in empty {
        let mut terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;

        if terminal
            .record(
                &SaleEvent::Abandoned {
                    sale_id,
                    abandoned_by,
                },
                new_id(),
                now(),
            )
            .is_ok()
        {
            discarded = discarded.saturating_add(1);
        }
    }

    Ok(discarded)
}

// -------------------------------------------------------------------------------------------
// Splitting a bill
// -------------------------------------------------------------------------------------------

/// One person's share.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitPartView {
    pub number: u32,
    pub amount_minor: i64,
    /// The lines this part covers. Empty for an even split, where nobody pays for anything named.
    pub line_ids: Vec<Uuid>,
}

/// Work out what each share of a bill should be.
///
/// A split is arithmetic, not a new kind of transaction: three people paying separately is three
/// tenders against one sale, which the sale has supported since P1. Nothing is recorded here — the
/// ordinary tender path takes it from the amounts this returns.
///
/// `line_assignment` empty means split evenly `ways` times. Otherwise it gives, per part, the lines
/// that part is paying for, and every active line must appear exactly once.
#[tauri::command]
pub fn split_bill(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
    ways: u32,
    line_assignment: Vec<Vec<Uuid>>,
) -> Result<Vec<SplitPartView>, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    let sale = terminal.sale(sale_id)?;
    let totals = sale.totals().map_err(|error| CommandError {
        code: "rejected",
        message: error.to_string(),
    })?;

    let parts = if line_assignment.is_empty() {
        sahl_core::sale::evenly(totals.total, ways).map_err(|error| CommandError {
            code: "rejected",
            message: error.to_string(),
        })?
    } else {
        // Line totals come from the calculated order rather than being recomputed, so an
        // apportioned order discount lands exactly where the tax engine put it.
        let line_totals: Vec<sahl_core::Money> =
            totals.lines.iter().map(|line| line.total).collect();

        // `totals.lines` covers active lines only, so the voided ones are filtered out of the
        // aggregate's list too — otherwise the two would be misaligned and a split would charge
        // one line's money against another's id.
        let active: Vec<sahl_core::sale::SaleLine> = sale.active_lines().cloned().collect();

        sahl_core::sale::by_lines(&active, &line_totals, &line_assignment).map_err(|error| {
            CommandError {
                code: "bad_split",
                message: error.to_string(),
            }
        })?
    };

    Ok(parts
        .into_iter()
        .map(|part| SplitPartView {
            number: part.number,
            amount_minor: part.amount.minor(),
            line_ids: part.line_ids,
        })
        .collect())
}

// -------------------------------------------------------------------------------------------
// The kitchen
// -------------------------------------------------------------------------------------------

/// One station's instruction, as a screen shows it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KitchenTicketView {
    pub station: &'static str,
    /// `order` or `cancellation` — never conflated, because a cancellation read as an order gets
    /// the dish made twice.
    pub kind: &'static str,
    pub table_label: Option<String>,
    pub covers: Option<u32>,
    pub round: u32,
    pub lines: Vec<KitchenLineView>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KitchenLineView {
    pub name: String,
    pub quantity_milli: i64,
    pub modifiers: Vec<String>,
}

/// What each station has not yet been told about this ticket.
#[tauri::command]
pub fn pending_kitchen(
    state: tauri::State<'_, TerminalState>,
    sale_id: Uuid,
) -> Result<Vec<KitchenTicketView>, CommandError> {
    let terminal = state.inner.lock().map_err(|_| CommandError {
        code: "poisoned",
        message: "the till is in an inconsistent state and must be restarted".to_owned(),
    })?;

    Ok(terminal
        .pending_kitchen(sale_id)?
        .into_iter()
        .map(ticket_view)
        .collect())
}

/// What happened when an order went to the stations.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FireOutcome {
    /// Tickets sent, or that would have been sent had a printer been configured.
    pub tickets: Vec<KitchenTicketView>,
    pub printed: bool,
    /// Why not, when it did not. The order is recorded either way.
    pub reason: Option<String>,
}

/// Send everything new to its station.
///
/// Records the firing **before** printing, and does not undo it if the printer fails. That ordering
/// is deliberate and it is the opposite of what feels natural: a paper jam that rolled back the
/// record would mean the next press reprints lines a station may already have on a half-printed
/// slip, and the kitchen makes them twice. A recorded firing that failed to print is recoverable —
/// a waiter walks over and says four covers of curry — while a duplicate is food in the bin.
#[tauri::command]
pub fn fire_kitchen(
    state: tauri::State<'_, TerminalState>,
    printer: tauri::State<'_, PrinterTarget>,
    sale_id: Uuid,
    printed_at: String,
    paper: String,
    fired_by: Uuid,
) -> Result<FireOutcome, CommandError> {
    let paper = match paper.as_str() {
        "mm58" => PaperWidth::Mm58,
        "mm80" => PaperWidth::Mm80,
        other => {
            return Err(CommandError {
                code: "bad_paper",
                message: format!("{other} is not a paper width"),
            });
        }
    };

    let tickets = {
        let mut terminal = state.inner.lock().map_err(|_| CommandError {
            code: "poisoned",
            message: "the till is in an inconsistent state and must be restarted".to_owned(),
        })?;

        let tickets = terminal.pending_kitchen(sale_id)?;
        if tickets.is_empty() {
            return Ok(FireOutcome {
                tickets: Vec::new(),
                printed: true,
                reason: None,
            });
        }

        let round = tickets.first().map_or(1, |ticket| ticket.round);
        let line_ids: Vec<Uuid> = tickets
            .iter()
            .filter(|ticket| ticket.kind == sahl_core::kitchen::TicketKind::Order)
            .flat_map(|ticket| ticket.lines.iter().map(|line| line.line_id))
            .collect();

        if !line_ids.is_empty() {
            terminal.record(
                &SaleEvent::LinesFired {
                    sale_id,
                    line_ids,
                    round,
                    at: now(),
                    fired_by,
                },
                new_id(),
                now(),
            )?;
        }
        tickets
    };

    let mut printed = true;
    let mut reason = None;
    for ticket in &tickets {
        let data = EscposKitchenTicket {
            station: ticket.station.heading().to_owned(),
            is_cancellation: ticket.kind == sahl_core::kitchen::TicketKind::Cancellation,
            table_label: ticket.table_label.clone(),
            covers: ticket.covers,
            round: ticket.round,
            printed_at: printed_at.clone(),
            lines: ticket
                .lines
                .iter()
                .map(|line| EscposKitchenLine {
                    name: line.name.clone(),
                    quantity: Quantity::from_milli(line.quantity_milli),
                    modifiers: line.modifiers.clone(),
                })
                .collect(),
        };

        let job = EscposDocument::render_kitchen(&data, paper);
        if let Err(error) = crate::printer::print(&printer, job.bytes()) {
            printed = false;
            reason = Some(error.to_string());
        }
    }

    Ok(FireOutcome {
        tickets: tickets.into_iter().map(ticket_view).collect(),
        printed,
        reason,
    })
}

fn ticket_view(ticket: sahl_core::kitchen::KitchenTicket) -> KitchenTicketView {
    KitchenTicketView {
        station: ticket.station.label(),
        kind: match ticket.kind {
            sahl_core::kitchen::TicketKind::Order => "order",
            sahl_core::kitchen::TicketKind::Cancellation => "cancellation",
        },
        table_label: ticket.table_label,
        covers: ticket.covers,
        round: ticket.round,
        lines: ticket
            .lines
            .into_iter()
            .map(|line| KitchenLineView {
                name: line.name,
                quantity_milli: line.quantity_milli,
                modifiers: line.modifiers,
            })
            .collect(),
    }
}
