//! The audit trail.
//!
//! Nothing here records anything — the log already did. This turns events that were written for
//! their own sake into the handful of lines an owner should actually read, which is a different
//! problem: a feed showing everything is a feed nobody opens.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::Money;
use crate::sale::{SaleEvent, VoidReason};
use crate::shift::{CashMovementReason, ShiftEvent};
use crate::staff::role::{ApprovalPolicy, Permission, Role, authorize_discount, authorize_void};
use crate::time::Timestamp;

/// How much attention a line deserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Ordinary, but worth being able to find later.
    Routine,
    /// Worth a glance in the daily digest.
    Notable,
    /// Worth interrupting someone about.
    Alert,
}

/// What happened, who did it, and who allowed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at: Timestamp,
    pub severity: Severity,
    /// Stable machine-readable kind, for filtering and counting.
    pub kind: &'static str,
    /// The person whose action it was.
    pub actor: Uuid,
    /// Who approved it, when approval was needed. Equal to `actor` means self-approved, which is
    /// itself the thing worth noticing.
    pub approved_by: Option<Uuid>,
    pub amount: Option<Money>,
    /// One line, for a human.
    pub summary: String,
}

impl AuditEntry {
    /// Whether the person who did it also approved it.
    ///
    /// Not automatically wrong — an owner legitimately approves their own void — but on a cashier's
    /// account it means the approval step was bypassed, which is exactly what the split of duties
    /// exists to prevent.
    #[must_use]
    pub fn is_self_approved(&self) -> bool {
        self.approved_by == Some(self.actor)
    }
}

/// Extract auditable lines from sale events.
///
/// Only the actions that move money without selling something. Line additions and tenders are the
/// ordinary business of a till and would drown the feed.
#[must_use]
pub fn from_sales(events: &[(SaleEvent, Timestamp, Uuid)]) -> Vec<AuditEntry> {
    // What each line was worth, so a void can carry its own value. Without it the feed cannot
    // judge a void against the outlet's threshold, and every under-limit void by a cashier would
    // read as an authority they did not have.
    let mut line_values: std::collections::BTreeMap<Uuid, Money> =
        std::collections::BTreeMap::new();
    for (event, _, _) in events {
        if let SaleEvent::LineAdded {
            line_id,
            unit_price,
            quantity,
            modifiers,
            ..
        } = event
        {
            let mut price = *unit_price;
            for modifier in modifiers {
                price = price.checked_add(modifier.price_delta).unwrap_or(price);
            }
            if let Ok(value) = price.mul_ratio(
                quantity.milli(),
                crate::quantity::Quantity::MILLI_PER_UNIT,
                crate::money::Rounding::HalfUp,
            ) {
                line_values.insert(*line_id, value);
            }
        }
    }

    events
        .iter()
        .filter_map(|(event, at, actor)| match event {
            SaleEvent::LineVoided {
                line_id,
                reason,
                authorized_by,
                ..
            } => Some(AuditEntry {
                at: *at,
                // Every void is notable. Whether a *pattern* of them is an alert is a question for
                // the digest, which can see across a shift; a single line cannot.
                severity: Severity::Notable,
                kind: "sale.line_voided",
                actor: *actor,
                approved_by: Some(*authorized_by),
                amount: line_values.get(line_id).copied(),
                summary: format!("Line voided ({})", void_label(*reason)),
            }),

            SaleEvent::OrderDiscounted {
                discount,
                authorized_by,
                ..
            } => Some(AuditEntry {
                at: *at,
                severity: Severity::Notable,
                kind: "sale.order_discounted",
                actor: *actor,
                approved_by: Some(*authorized_by),
                amount: discount_amount(discount),
                summary: "Discount applied to a whole sale".to_owned(),
            }),

            SaleEvent::Abandoned { .. } => Some(AuditEntry {
                at: *at,
                severity: Severity::Routine,
                kind: "sale.abandoned",
                actor: *actor,
                approved_by: None,
                amount: None,
                summary: "Ticket abandoned without payment".to_owned(),
            }),

            _ => None,
        })
        .collect()
}

/// Extract auditable lines from shift events.
#[must_use]
pub fn from_shifts(events: &[(ShiftEvent, Uuid)]) -> Vec<AuditEntry> {
    events
        .iter()
        .filter_map(|(event, actor)| match event {
            ShiftEvent::CashMoved {
                amount,
                reason,
                authorized_by,
                at,
                ..
            } => Some(AuditEntry {
                at: *at,
                severity: cash_severity(*reason),
                kind: "shift.cash_moved",
                actor: *actor,
                approved_by: Some(*authorized_by),
                amount: Some(*amount),
                summary: format!("Cash {}", cash_label(*reason)),
            }),

            // The counter is named on the event itself, so it is the actor regardless of whose
            // session it was — a manager counting a cashier's drawer is the ordinary case.
            ShiftEvent::Counted {
                counted,
                counted_by,
                at,
                ..
            } => Some(AuditEntry {
                at: *at,
                severity: Severity::Routine,
                kind: "shift.counted",
                actor: *counted_by,
                approved_by: None,
                amount: Some(*counted),
                summary: "Drawer counted".to_owned(),
            }),

            _ => None,
        })
        .collect()
}

/// Order a feed for reading: most severe first, then most recent.
///
/// Severity before time on purpose. A feed sorted only by time buries an alert from this morning
/// under a hundred routine lines from this afternoon.
#[must_use]
pub fn ranked(mut entries: Vec<AuditEntry>) -> Vec<AuditEntry> {
    entries.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(b.at.millis().cmp(&a.at.millis()))
            .then(a.kind.cmp(b.kind))
    });
    entries
}

/// Lines where the actor approved their own action.
///
/// The raw fact, not a judgement — a manager authorising their own cash lift lands here and is
/// entirely ordinary. See [`unapproved`] for the version that knows the difference.
#[must_use]
pub fn self_approved(entries: &[AuditEntry]) -> Vec<&AuditEntry> {
    entries
        .iter()
        .filter(|entry| entry.is_self_approved())
        .collect()
}

/// Self-approved lines where the actor's own role did not carry that authority.
///
/// This is the signal worth waking someone for. `role_of` resolves a staff id, because roles live
/// in the user table rather than the event log — the log records who, and the answer to whether
/// that was allowed can change after the fact when someone is promoted or demoted.
///
/// An unresolvable id counts as unapproved: a deleted or unknown actor approving their own void is
/// more alarming than a known one, not less.
#[must_use]
pub fn unapproved<'a, F>(
    entries: &'a [AuditEntry],
    role_of: F,
    policy: &ApprovalPolicy,
) -> Vec<&'a AuditEntry>
where
    F: Fn(Uuid) -> Option<Role>,
{
    entries
        .iter()
        .filter(|entry| entry.is_self_approved())
        .filter(|entry| {
            role_of(entry.actor).is_none_or(|role| !permits(role, entry.kind, entry.amount, policy))
        })
        .collect()
}

/// Whether a role carried the authority an entry of this kind needed.
///
/// Matches on `kind` rather than re-deriving from the event, so the feed can judge entries read
/// back from storage where the original event is no longer to hand.
///
/// **The threshold is part of the answer.** A cashier giving a discount inside the outlet's limit
/// did so on their own authority and is recorded as their own approver — judging that against the
/// blanket permission alone would put every legitimate one in the alert feed, which is both wrong
/// and the fastest way to make an owner stop reading it.
///
/// An action whose amount was never recorded falls back to the blanket permission. That is the
/// stricter reading, and the safer direction for an unknown to fall.
fn permits(role: Role, kind: &str, amount: Option<Money>, policy: &ApprovalPolicy) -> bool {
    match kind {
        "sale.line_voided" => amount.map_or_else(
            || role.can(Permission::VoidLine),
            |value| authorize_void(role, value, policy).is_allowed(),
        ),
        "sale.order_discounted" => amount.map_or_else(
            || role.can(Permission::ApplyDiscount),
            |value| authorize_discount(role, value, policy).is_allowed(),
        ),
        "shift.cash_moved" => role.can(Permission::MoveCash),
        // Nothing else in the feed is an approval-bearing action.
        _ => true,
    }
}

/// A correction is the only cash movement that means "the numbers were wrong", so it is the only
/// one worth interrupting someone about on its own.
const fn cash_severity(reason: CashMovementReason) -> Severity {
    match reason {
        CashMovementReason::Correction => Severity::Alert,
        CashMovementReason::Skim | CashMovementReason::PettyCash | CashMovementReason::Refund => {
            Severity::Notable
        }
        CashMovementReason::FloatTopUp => Severity::Routine,
    }
}

const fn cash_label(reason: CashMovementReason) -> &'static str {
    match reason {
        CashMovementReason::FloatTopUp => "added to the float",
        CashMovementReason::Skim => "lifted to the safe",
        CashMovementReason::PettyCash => "paid out as petty cash",
        CashMovementReason::Refund => "refunded outside a sale",
        CashMovementReason::Correction => "corrected",
    }
}

const fn void_label(reason: VoidReason) -> &'static str {
    match reason {
        VoidReason::Mistake => "rung in error",
        VoidReason::CustomerChanged => "customer changed their mind",
        VoidReason::Damaged => "damaged",
        VoidReason::Unavailable => "unavailable",
    }
}

fn discount_amount(discount: &crate::tax::Discount) -> Option<Money> {
    match discount {
        crate::tax::Discount::Amount { amount } => Some(*amount),
        // A percentage has no fixed value until it meets a basket, so reporting one here would be
        // a number the owner could not reconcile against anything.
        crate::tax::Discount::Percentage { .. } | crate::tax::Discount::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(n: i64) -> Timestamp {
        Timestamp::from_millis(1_753_000_000_000 + n * 60_000)
    }

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, Currency::Bdt)
    }

    const CASHIER: u128 = 0xCA;
    const MANAGER: u128 = 0x11A;

    fn voided(authorized_by: u128, minute: i64) -> (SaleEvent, Timestamp, Uuid) {
        (
            SaleEvent::LineVoided {
                sale_id: id(1),
                line_id: id(2),
                reason: VoidReason::Mistake,
                authorized_by: id(authorized_by),
            },
            at(minute),
            id(CASHIER),
        )
    }

    /// The line the `voided` fixture strikes off, worth 100.00.
    fn added(minute: i64) -> (SaleEvent, Timestamp, Uuid) {
        (
            SaleEvent::LineAdded {
                sale_id: id(1),
                line_id: id(2),
                product_id: id(3),
                name: "Item".to_owned(),
                unit_price: bdt(10_000),
                quantity: crate::quantity::Quantity::ONE,
                tax_class: crate::tax::TaxClass::standard(1500),
                modifiers: Vec::new(),
            },
            at(minute),
            id(CASHIER),
        )
    }

    fn cash(reason: CashMovementReason, minor: i64, minute: i64) -> (ShiftEvent, Uuid) {
        (
            ShiftEvent::CashMoved {
                shift_id: id(9),
                movement_id: id(10),
                amount: bdt(minor),
                reason,
                note: None,
                authorized_by: id(MANAGER),
                at: at(minute),
            },
            id(CASHIER),
        )
    }

    #[test]
    fn ordinary_selling_does_not_reach_the_feed() {
        // A feed showing everything is a feed nobody opens.
        let events = vec![(
            SaleEvent::TenderRecorded {
                sale_id: id(1),
                tender_id: id(2),
                method: crate::sale::TenderMethod::Cash,
                amount: bdt(1_000),
                reference: None,
            },
            at(0),
            id(CASHIER),
        )];
        assert!(from_sales(&events).is_empty());
    }

    #[test]
    fn a_void_records_who_approved_it() {
        let entries = from_sales(&[voided(MANAGER, 0)]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].actor, id(CASHIER));
        assert_eq!(entries[0].approved_by, Some(id(MANAGER)));
        assert!(!entries[0].is_self_approved());
    }

    #[test]
    fn a_self_approved_void_is_visible_as_such() {
        // On a cashier's account this means the approval step was bypassed — exactly what the
        // split of duties exists to prevent.
        let entries = from_sales(&[voided(CASHIER, 0)]);

        assert!(entries[0].is_self_approved());
        assert_eq!(self_approved(&entries).len(), 1);
    }

    #[test]
    fn a_manager_approving_their_own_cash_lift_is_not_flagged() {
        // Self-approval is the raw fact; it is only a finding when the role did not carry it.
        let entries = from_shifts(&[(
            ShiftEvent::CashMoved {
                shift_id: id(9),
                movement_id: id(10),
                amount: bdt(-100_000),
                reason: CashMovementReason::Skim,
                note: None,
                authorized_by: id(MANAGER),
                at: at(0),
            },
            id(MANAGER),
        )]);

        assert_eq!(self_approved(&entries).len(), 1, "the fact");
        assert!(
            unapproved(&entries, |_| Some(Role::Manager), &STRICT).is_empty(),
            "but not a finding"
        );
    }

    /// Nothing may be done unaided, which is how an unconfigured outlet reads.
    const STRICT: ApprovalPolicy = ApprovalPolicy {
        discount_limit: Money::from_minor(0, Currency::Bdt),
        discount_rate_limit: crate::money::Rate::ZERO,
        void_limit: Money::from_minor(0, Currency::Bdt),
    };

    #[test]
    fn a_cashier_approving_their_own_void_is_flagged() {
        let entries = from_sales(&[voided(CASHIER, 0)]);
        assert_eq!(
            unapproved(&entries, |_| Some(Role::Cashier), &STRICT).len(),
            1
        );
    }

    #[test]
    fn a_void_inside_the_outlets_limit_is_not_an_authority_the_cashier_lacked() {
        // The threshold is part of the answer. Judging it against the blanket permission alone
        // would put every legitimate under-limit void in the alert feed.
        const LENIENT: ApprovalPolicy = ApprovalPolicy {
            discount_limit: Money::from_minor(50_000, Currency::Bdt),
            discount_rate_limit: crate::money::Rate::ZERO,
            void_limit: Money::from_minor(50_000, Currency::Bdt),
        };

        let entries = from_sales(&[added(0), voided(CASHIER, 1)]);
        assert_eq!(entries[0].amount, Some(bdt(10_000)));
        assert!(unapproved(&entries, |_| Some(Role::Cashier), &LENIENT).is_empty());
    }

    #[test]
    fn a_void_carries_the_value_of_the_line_that_was_struck_off() {
        // Both so the threshold can be judged and so an owner reading the feed sees what it cost.
        let entries = from_sales(&[added(0), voided(CASHIER, 1)]);
        assert_eq!(entries[0].amount, Some(bdt(10_000)));
    }

    #[test]
    fn an_unknown_actor_is_flagged() {
        // A deleted or unrecognised actor approving their own void is more alarming, not less.
        let entries = from_sales(&[voided(CASHIER, 0)]);
        assert_eq!(unapproved(&entries, |_| None, &STRICT).len(), 1);
    }

    #[test]
    fn a_correction_outranks_a_skim() {
        // A correction is the only cash movement that means the numbers were wrong.
        let entries = from_shifts(&[
            cash(CashMovementReason::Skim, -20_000, 0),
            cash(CashMovementReason::Correction, -500, 1),
        ]);
        let ranked = ranked(entries);

        assert_eq!(ranked[0].severity, Severity::Alert);
        assert!(ranked[0].summary.contains("corrected"));
    }

    #[test]
    fn a_float_top_up_stays_routine() {
        let entries = from_shifts(&[cash(CashMovementReason::FloatTopUp, 50_000, 0)]);
        assert_eq!(entries[0].severity, Severity::Routine);
    }

    #[test]
    fn ranking_puts_severity_before_recency() {
        // A feed sorted only by time buries this morning's alert under this afternoon's noise.
        let mut entries = from_shifts(&[cash(CashMovementReason::Correction, -500, 0)]);
        entries.extend(from_shifts(&[cash(
            CashMovementReason::FloatTopUp,
            50_000,
            600,
        )]));

        let ranked = ranked(entries);
        assert_eq!(ranked[0].severity, Severity::Alert, "older, but louder");
        assert_eq!(ranked[1].severity, Severity::Routine);
    }

    #[test]
    fn equal_severity_falls_back_to_most_recent() {
        let entries = ranked(from_sales(&[voided(MANAGER, 0), voided(MANAGER, 10)]));
        assert_eq!(entries[0].at, at(10));
    }

    #[test]
    fn a_fixed_discount_reports_its_value_and_a_percentage_does_not() {
        // A percentage has no value until it meets a basket; reporting one would give the owner a
        // number they cannot reconcile against anything.
        let fixed = from_sales(&[(
            SaleEvent::OrderDiscounted {
                sale_id: id(1),
                discount: crate::tax::Discount::Amount { amount: bdt(5_000) },
                authorized_by: id(MANAGER),
            },
            at(0),
            id(CASHIER),
        )]);
        assert_eq!(fixed[0].amount, Some(bdt(5_000)));

        let percentage = from_sales(&[(
            SaleEvent::OrderDiscounted {
                sale_id: id(1),
                discount: crate::tax::Discount::Percentage {
                    rate: crate::money::Rate::from_basis_points(1000),
                },
                authorized_by: id(MANAGER),
            },
            at(0),
            id(CASHIER),
        )]);
        assert_eq!(percentage[0].amount, None);
    }

    #[test]
    fn ranking_is_deterministic() {
        // This reaches a report; two devices must produce the same order.
        let entries = from_sales(&[voided(MANAGER, 5), voided(CASHIER, 5)]);
        assert_eq!(ranked(entries.clone()), ranked(entries));
    }
}
