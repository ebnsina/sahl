//! What the webview is given.
//!
//! Every monetary field here is an **exact integer of minor units**, already computed by
//! `sahl-core`. The UI's only job is to hand it to `Intl.NumberFormat`. Nothing here is a
//! pre-formatted string, because a formatted string cannot be re-formatted for another locale, and
//! nothing here is a float, because a float would undo the entire money design at the last step.
//!
//! These are also the reason the webview needs no database access: it cannot ask a question these
//! views do not answer, so there is no path around `sahl-core`.

use sahl_core::sale::{Sale, SaleError, SaleStatus};
use serde::Serialize;
use uuid::Uuid;

/// One line as the sell screen shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineView {
    pub id: Uuid,
    pub name: String,
    /// Thousandths of a unit — 1234 is 1.234 kg.
    pub quantity_milli: i64,
    pub unit_price_minor: i64,
    /// What this line contributes to the total, after its own and its apportioned discount.
    pub total_minor: i64,
    pub tax_minor: i64,
    /// A voided line stays in the list, struck through. Hiding it would hide the evidence.
    pub voided: bool,
}

/// A tax-summary row, as printed on the receipt.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxGroupView {
    /// Rate in basis points; `1500` is 15%. The UI renders it with `Intl`.
    pub basis_points: i32,
    /// `standard`, `zero_rated`, or `exempt` — legally distinct even when the rate matches.
    pub class: &'static str,
    pub taxable_base_minor: i64,
    pub tax_minor: i64,
}

/// A tender already taken against the sale.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TenderView {
    pub id: Uuid,
    pub method: String,
    pub amount_minor: i64,
}

/// The whole sell screen in one payload.
///
/// Returned by every mutating command, so the UI never has to reconstruct state from a delta or
/// re-fetch after an action — a round trip it would sometimes skip, and then drift.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaleView {
    pub id: Uuid,
    pub status: &'static str,
    pub currency: &'static str,
    pub lines: Vec<LineView>,
    pub tax_groups: Vec<TaxGroupView>,
    pub tenders: Vec<TenderView>,
    pub gross_minor: i64,
    pub discount_minor: i64,
    pub net_minor: i64,
    pub tax_minor: i64,
    pub total_minor: i64,
    pub tendered_minor: i64,
    /// Positive means still owed; the sell screen shows this until it reaches zero.
    pub balance_due_minor: i64,
    pub change_due_minor: i64,
    pub void_count: usize,
    pub needs_drawer: bool,
}

impl SaleView {
    /// Build the view for a sale.
    ///
    /// # Errors
    /// [`SaleError`] if the sale's totals cannot be computed.
    pub fn of(sale: &Sale) -> Result<Self, SaleError> {
        let status = match sale.status() {
            SaleStatus::Open => "open",
            SaleStatus::Completed => "completed",
            SaleStatus::Abandoned => "abandoned",
        };

        // An empty or fully-voided sale has no totals to compute, but the screen must still render
        // — a cashier who voids their only line should see an empty cart, not an error.
        let Ok(totals) = sale.totals() else {
            return Ok(Self {
                id: sale.id(),
                status,
                currency: sale.currency().code(),
                lines: sale.lines().iter().map(empty_line).collect(),
                tax_groups: Vec::new(),
                tenders: tenders_of(sale),
                gross_minor: 0,
                discount_minor: 0,
                net_minor: 0,
                tax_minor: 0,
                total_minor: 0,
                tendered_minor: sale.tenders().iter().map(|t| t.amount.minor()).sum(),
                balance_due_minor: 0,
                change_due_minor: 0,
                void_count: sale.void_count(),
                needs_drawer: sale.needs_drawer(),
            });
        };

        // Line totals come back in the order the active lines were submitted, so they are zipped
        // back onto the active lines rather than indexed — voided lines have no entry.
        let mut computed = totals.lines.iter();
        let lines = sale
            .lines()
            .iter()
            .map(|line| {
                if line.is_active() {
                    computed.next().map_or_else(
                        || empty_line(line),
                        |totals| LineView {
                            id: line.id,
                            name: line.name.clone(),
                            quantity_milli: line.quantity.milli(),
                            unit_price_minor: line.unit_price.minor(),
                            total_minor: totals.total.minor(),
                            tax_minor: totals.tax.minor(),
                            voided: false,
                        },
                    )
                } else {
                    empty_line(line)
                }
            })
            .collect();

        Ok(Self {
            id: sale.id(),
            status,
            currency: sale.currency().code(),
            lines,
            tax_groups: totals
                .tax_groups
                .iter()
                .map(|group| TaxGroupView {
                    basis_points: group.tax_class.rate().basis_points(),
                    class: class_label(group.tax_class),
                    taxable_base_minor: group.taxable_base.minor(),
                    tax_minor: group.tax.minor(),
                })
                .collect(),
            tenders: tenders_of(sale),
            gross_minor: totals.gross.minor(),
            discount_minor: totals.discount.minor(),
            net_minor: totals.net.minor(),
            tax_minor: totals.tax.minor(),
            total_minor: totals.total.minor(),
            tendered_minor: sale.tendered().map(|m| m.minor()).unwrap_or(0),
            balance_due_minor: sale.balance_due().map(|m| m.minor()).unwrap_or(0),
            change_due_minor: sale.change_due().map(|m| m.minor()).unwrap_or(0),
            void_count: sale.void_count(),
            needs_drawer: sale.needs_drawer(),
        })
    }
}

fn empty_line(line: &sahl_core::sale::SaleLine) -> LineView {
    LineView {
        id: line.id,
        name: line.name.clone(),
        quantity_milli: line.quantity.milli(),
        unit_price_minor: line.unit_price.minor(),
        total_minor: 0,
        tax_minor: 0,
        voided: !line.is_active(),
    }
}

fn tenders_of(sale: &Sale) -> Vec<TenderView> {
    sale.tenders()
        .iter()
        .map(|tender| TenderView {
            id: tender.id,
            method: method_label(tender.method),
            amount_minor: tender.amount.minor(),
        })
        .collect()
}

const fn class_label(class: sahl_core::tax::TaxClass) -> &'static str {
    match class {
        sahl_core::tax::TaxClass::Standard { .. } => "standard",
        sahl_core::tax::TaxClass::ZeroRated => "zero_rated",
        sahl_core::tax::TaxClass::Exempt => "exempt",
    }
}

fn method_label(method: sahl_core::sale::TenderMethod) -> String {
    use sahl_core::sale::TenderMethod as M;
    match method {
        M::Cash => "cash".to_owned(),
        M::Card => "card".to_owned(),
        M::MobileWallet { wallet } => format!("{wallet:?}").to_lowercase(),
        M::BankTransfer => "bank_transfer".to_owned(),
        M::StoreCredit => "store_credit".to_owned(),
        // TenderMethod is #[non_exhaustive], so the compiler cannot force this arm to be updated
        // when a method is added. Rendering an honest "unknown" beats guessing: a tender the UI
        // cannot name is one a cashier should query, not one it should quietly mislabel as cash.
        _ => "unknown".to_owned(),
    }
}
