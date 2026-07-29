//! A shift's worth of authority decisions, and what the owner reads afterwards.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic
)]

use sahl_core::Timestamp;
use sahl_core::money::{Currency, Money, Rate};
use sahl_core::sale::{SaleEvent, TenderMethod, VoidReason};
use sahl_core::shift::{CashMovementReason, ShiftEvent};
use sahl_core::staff::{
    ApprovalPolicy, Authorization, Permission, Role, Severity, authorize, authorize_discount,
    authorize_void, from_sales, from_shifts, pin, ranked, self_approved, unapproved,
};
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn at(minute: i64) -> Timestamp {
    Timestamp::from_millis(1_753_000_000_000 + minute * 60_000)
}

fn bdt(minor: i64) -> Money {
    Money::from_minor(minor, BDT)
}

const RUMA: u128 = 0xCA; // cashier
const HABIB: u128 = 0x11A; // manager

fn policy() -> ApprovalPolicy {
    ApprovalPolicy {
        discount_limit: bdt(5_000),
        discount_rate_limit: Rate::from_basis_points(1000),
        void_limit: bdt(10_000),
    }
}

#[test]
fn an_evening_at_the_counter_leaves_a_readable_trail() {
    // Ruma rings sales, voids a small line herself, needs Habib for a large one, and Habib lifts
    // cash to the safe. Everything that moved money without selling something should surface.
    let sales = vec![
        (
            SaleEvent::TenderRecorded {
                sale_id: id(1),
                tender_id: id(2),
                method: TenderMethod::Cash,
                amount: bdt(45_000),
                reference: None,
            },
            at(0),
            id(RUMA),
        ),
        (
            SaleEvent::LineVoided {
                sale_id: id(1),
                line_id: id(3),
                reason: VoidReason::Mistake,
                authorized_by: id(RUMA),
            },
            at(5),
            id(RUMA),
        ),
        (
            SaleEvent::LineVoided {
                sale_id: id(4),
                line_id: id(5),
                reason: VoidReason::CustomerChanged,
                authorized_by: id(HABIB),
            },
            at(90),
            id(RUMA),
        ),
    ];

    let shifts = vec![(
        ShiftEvent::CashMoved {
            shift_id: id(9),
            movement_id: id(10),
            amount: bdt(-100_000),
            reason: CashMovementReason::Skim,
            note: None,
            authorized_by: id(HABIB),
            at: at(120),
        },
        id(HABIB),
    )];

    let mut feed = from_sales(&sales);
    feed.extend(from_shifts(&shifts));

    // The tender does not appear. Three actions did.
    assert_eq!(feed.len(), 3);

    // Two lines are self-approved: Ruma's small void and Habib's own cash lift. Only one of them
    // is a finding — a manager authorising their own skim is the ordinary way a safe drop happens.
    assert_eq!(self_approved(&feed).len(), 2);

    let flagged = unapproved(&feed, |actor| match actor.as_u128() {
        RUMA => Some(Role::Cashier),
        HABIB => Some(Role::Manager),
        _ => None,
    });

    assert_eq!(flagged.len(), 1);
    assert_eq!(flagged[0].actor, id(RUMA));
    assert_eq!(flagged[0].kind, "sale.line_voided");
}

#[test]
fn the_feed_leads_with_what_matters_not_what_is_newest() {
    let entries = ranked(from_shifts(&[
        (
            ShiftEvent::CashMoved {
                shift_id: id(9),
                movement_id: id(10),
                amount: bdt(-700),
                reason: CashMovementReason::Correction,
                note: Some("drawer short".to_owned()),
                authorized_by: id(HABIB),
                at: at(10),
            },
            id(HABIB),
        ),
        (
            ShiftEvent::Counted {
                shift_id: id(9),
                counted: bdt(310_000),
                counted_by: id(HABIB),
                at: at(480),
            },
            id(RUMA),
        ),
    ]));

    assert_eq!(entries[0].severity, Severity::Alert);
    assert_eq!(entries[0].kind, "shift.cash_moved");
    assert_eq!(entries[1].severity, Severity::Routine);
    assert_eq!(entries[1].actor, id(HABIB), "the counter, not the session");
}

#[test]
fn the_thresholds_and_the_permissions_agree_on_what_a_cashier_may_do() {
    // A cashier has no blanket void or discount permission, but the policy grants small ones. If
    // these two disagreed, either every void would need a manager or none would.
    assert!(!Role::Cashier.can(Permission::VoidLine));
    assert!(authorize_void(Role::Cashier, bdt(4_000), &policy()).is_allowed());
    assert!(authorize_discount(Role::Cashier, bdt(2_000), &policy()).is_allowed());

    assert_eq!(
        authorize_void(Role::Cashier, bdt(60_000), &policy()),
        Authorization::NeedsApproval {
            minimum: Role::Manager
        }
    );
}

#[test]
fn a_refusal_names_someone_who_can_help() {
    // "Get a manager" is actionable at a counter mid-queue; "denied" is not.
    for permission in [
        Permission::RefundSale,
        Permission::NoSaleDrawer,
        Permission::OverridePrice,
        Permission::MoveCash,
        Permission::CloseShift,
    ] {
        match authorize(Role::Cashier, permission) {
            Authorization::NeedsApproval { minimum } => assert!(minimum > Role::Cashier),
            other => panic!("{permission:?} gave {other:?}"),
        }
    }
}

#[test]
fn role_names_match_what_the_database_stores() {
    // The `app_user.role` CHECK constraint lists these exact strings; a rename here fails inserts
    // at runtime rather than at compile time.
    for (role, stored) in [
        (Role::Owner, "owner"),
        (Role::Manager, "manager"),
        (Role::Cashier, "cashier"),
    ] {
        assert_eq!(role.label(), stored);
        assert_eq!(
            serde_json::to_string(&role).expect("serialises"),
            format!("\"{stored}\"")
        );
    }
}

#[test]
fn a_pin_survives_the_round_trip_the_terminal_will_make() {
    // The server hashes; the terminal verifies offline against the synced hash. Same code, so the
    // two cannot drift the way a reimplementation would.
    let salt = argon2::password_hash::SaltString::from_b64("c2FobGludGVncmF0aW9u")
        .expect("valid b64 salt");
    let stored = pin::hash("8317", &salt).expect("hashes");

    assert_eq!(pin::verify("8317", &stored), Ok(true));
    assert_eq!(pin::verify("7318", &stored), Ok(false));
    assert!(!pin::needs_rehash(&stored));
}
