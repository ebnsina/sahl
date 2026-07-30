//! Who may do what.
//!
//! The interesting permissions are not the ones that gate features — they are the ones that gate
//! *money leaving without a sale*. A void, a discount, a no-sale drawer open and a price override
//! are the four ways a till loses cash without anyone noticing, so each requires a decision about
//! who authorised it, recorded at the time.

use serde::{Deserialize, Serialize};

use crate::money::{Currency, Money, Rate};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Rings sales. The default, and deliberately the least trusted.
    Cashier,
    /// Runs a shift: authorises voids and discounts, closes the till.
    Manager,
    /// Everything, including staff and settings.
    Owner,
}

/// Something a person might try to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Permission {
    RingSale,
    /// Void a line before payment.
    VoidLine,
    /// Refund after payment — a separate, higher bar, because the money has already been taken.
    RefundSale,
    ApplyDiscount,
    /// Change a price at the till rather than in the catalogue.
    OverridePrice,
    /// Open the drawer without a sale. The purest cash-removal path there is.
    NoSaleDrawer,
    OpenShift,
    CloseShift,
    /// Move cash in or out of the drawer outside a sale.
    MoveCash,
    ReceiveStock,
    CountStock,
    EditCatalogue,
    ManageStaff,
    EnrolDevice,
    ViewReports,
}

impl Role {
    /// Whether this role may do `permission` at all, ignoring value thresholds.
    #[must_use]
    pub const fn can(self, permission: Permission) -> bool {
        use Permission as P;
        match self {
            // Everything.
            Self::Owner => true,

            Self::Manager => !matches!(permission, P::ManageStaff | P::EnrolDevice),

            // A cashier sells and counts. Every cash-removal path needs someone else, which is the
            // entire point of the split — a person who can both take money and erase the record of
            // it has no check on them at all.
            Self::Cashier => matches!(
                permission,
                P::RingSale | P::OpenShift | P::CountStock | P::ReceiveStock
            ),
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cashier => "cashier",
            Self::Manager => "manager",
            Self::Owner => "owner",
        }
    }
}

/// Thresholds above which a cashier needs someone else's approval.
///
/// Per outlet, because what counts as a large discount in a pharmacy is not what counts as one in
/// a café. Zero means always require approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    /// Discounts up to this may be given without approval.
    pub discount_limit: Money,
    /// Percentage discounts up to this may be given without approval.
    pub discount_rate_limit: Rate,
    /// Voids of lines up to this value may be done without approval.
    pub void_limit: Money,
}

impl ApprovalPolicy {
    /// Every threshold at zero: nothing may be done without approval.
    ///
    /// The strictest setting, which is the only safe thing to assume about a shop nobody has
    /// configured. Raising a limit is then a deliberate act by an owner rather than a number this
    /// code picked for them.
    #[must_use]
    pub const fn strictest(currency: Currency) -> Self {
        Self {
            discount_limit: Money::zero(currency),
            discount_rate_limit: Rate::ZERO,
            void_limit: Money::zero(currency),
        }
    }
}

/// What an action needs before it may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authorization {
    /// The actor may do it themselves.
    Allowed,
    /// Permitted, but someone more senior must approve and be recorded.
    NeedsApproval { minimum: Role },
    /// Not permitted at all, by anyone with this role.
    Denied,
}

impl Authorization {
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }

    #[must_use]
    pub const fn is_denied(self) -> bool {
        matches!(self, Self::Denied)
    }
}

/// Whether `actor` may perform `permission`.
#[must_use]
pub const fn authorize(actor: Role, permission: Permission) -> Authorization {
    if actor.can(permission) {
        Authorization::Allowed
    } else if matches!(
        permission,
        Permission::ManageStaff | Permission::EnrolDevice
    ) {
        // Only an owner ever does these, so a manager being refused is not "get approval", it is
        // "not your job".
        if matches!(actor, Role::Owner) {
            Authorization::Allowed
        } else {
            Authorization::NeedsApproval {
                minimum: Role::Owner,
            }
        }
    } else {
        Authorization::NeedsApproval {
            minimum: Role::Manager,
        }
    }
}

/// Whether a discount of this size may be given by `actor` unaided.
#[must_use]
pub fn authorize_discount(actor: Role, amount: Money, policy: &ApprovalPolicy) -> Authorization {
    // A cashier has no blanket discount permission — the threshold *is* their grant. Checking the
    // permission first would make the limit unreachable and every discount need a manager.
    if actor.can(Permission::ApplyDiscount) || amount.minor() <= policy.discount_limit.minor() {
        Authorization::Allowed
    } else {
        Authorization::NeedsApproval {
            minimum: Role::Manager,
        }
    }
}

/// Whether a percentage discount of this size may be given by `actor` unaided.
///
/// Separate from [`authorize_discount`] because a percentage is checked before it meets a basket —
/// "20% off" is the same decision whether the ticket is small or large.
#[must_use]
pub fn authorize_discount_rate(actor: Role, rate: Rate, policy: &ApprovalPolicy) -> Authorization {
    if actor.can(Permission::ApplyDiscount) || rate <= policy.discount_rate_limit {
        Authorization::Allowed
    } else {
        Authorization::NeedsApproval {
            minimum: Role::Manager,
        }
    }
}

/// Whether a void of a line worth this much may be done by `actor` unaided.
#[must_use]
pub fn authorize_void(actor: Role, line_value: Money, policy: &ApprovalPolicy) -> Authorization {
    if !actor.can(Permission::VoidLine) {
        // A cashier voiding a trivial line all evening is normal; voiding a large one is the
        // pattern worth catching, so the threshold applies even though the base permission does not.
        if line_value.minor() <= policy.void_limit.minor() {
            return Authorization::Allowed;
        }
        return Authorization::NeedsApproval {
            minimum: Role::Manager,
        };
    }
    Authorization::Allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, Currency::Bdt)
    }

    fn policy() -> ApprovalPolicy {
        ApprovalPolicy {
            discount_limit: bdt(5_000),
            discount_rate_limit: Rate::from_basis_points(1000),
            void_limit: bdt(10_000),
        }
    }

    #[test]
    fn a_cashier_can_sell_but_not_erase() {
        // The whole point of the split: someone who can both take money and erase the record of it
        // has no check on them.
        assert!(Role::Cashier.can(Permission::RingSale));
        assert!(!Role::Cashier.can(Permission::VoidLine));
        assert!(!Role::Cashier.can(Permission::RefundSale));
        assert!(!Role::Cashier.can(Permission::NoSaleDrawer));
        assert!(!Role::Cashier.can(Permission::OverridePrice));
        assert!(!Role::Cashier.can(Permission::MoveCash));
    }

    #[test]
    fn a_cashier_opens_a_shift_but_does_not_close_one() {
        // Closing is where the variance is decided, so it needs the person who is accountable.
        assert!(Role::Cashier.can(Permission::OpenShift));
        assert!(!Role::Cashier.can(Permission::CloseShift));
    }

    #[test]
    fn a_manager_runs_the_floor_but_does_not_manage_staff() {
        assert!(Role::Manager.can(Permission::VoidLine));
        assert!(Role::Manager.can(Permission::CloseShift));
        assert!(Role::Manager.can(Permission::MoveCash));
        assert!(!Role::Manager.can(Permission::ManageStaff));
        assert!(!Role::Manager.can(Permission::EnrolDevice));
    }

    #[test]
    fn an_owner_can_do_everything() {
        for permission in [
            Permission::RingSale,
            Permission::VoidLine,
            Permission::RefundSale,
            Permission::NoSaleDrawer,
            Permission::ManageStaff,
            Permission::EnrolDevice,
            Permission::ViewReports,
        ] {
            assert!(Role::Owner.can(permission), "owner denied {permission:?}");
        }
    }

    #[test]
    fn a_refused_action_says_who_could_approve_it() {
        // "Get a manager" is actionable at a counter; "denied" is not.
        assert_eq!(
            authorize(Role::Cashier, Permission::VoidLine),
            Authorization::NeedsApproval {
                minimum: Role::Manager
            }
        );
        assert_eq!(
            authorize(Role::Manager, Permission::EnrolDevice),
            Authorization::NeedsApproval {
                minimum: Role::Owner
            }
        );
    }

    #[test]
    fn a_small_discount_needs_nobody() {
        assert!(authorize_discount(Role::Cashier, bdt(2_000), &policy()).is_allowed());
    }

    #[test]
    fn a_large_discount_needs_a_manager() {
        assert_eq!(
            authorize_discount(Role::Cashier, bdt(50_000), &policy()),
            Authorization::NeedsApproval {
                minimum: Role::Manager
            }
        );
    }

    #[test]
    fn the_discount_threshold_does_not_bind_a_manager() {
        assert!(authorize_discount(Role::Manager, bdt(500_000), &policy()).is_allowed());
    }

    #[test]
    fn a_cashier_may_void_a_small_line_but_not_a_large_one() {
        // Voiding a trivial line all evening is ordinary; voiding a large one is the pattern.
        assert!(authorize_void(Role::Cashier, bdt(4_000), &policy()).is_allowed());
        assert_eq!(
            authorize_void(Role::Cashier, bdt(48_000), &policy()),
            Authorization::NeedsApproval {
                minimum: Role::Manager
            }
        );
    }

    #[test]
    fn a_percentage_discount_has_its_own_threshold() {
        // Checked before it meets a basket: "20% off" is the same decision on any ticket.
        assert!(
            authorize_discount_rate(Role::Cashier, Rate::from_basis_points(500), &policy())
                .is_allowed()
        );
        assert_eq!(
            authorize_discount_rate(Role::Cashier, Rate::from_basis_points(2500), &policy()),
            Authorization::NeedsApproval {
                minimum: Role::Manager
            }
        );
    }

    #[test]
    fn a_zero_threshold_means_always_ask() {
        let strict = ApprovalPolicy {
            discount_limit: bdt(0),
            discount_rate_limit: Rate::ZERO,
            void_limit: bdt(0),
        };
        assert!(!authorize_discount(Role::Cashier, bdt(1), &strict).is_allowed());
        assert!(
            !authorize_discount_rate(Role::Cashier, Rate::from_basis_points(1), &strict)
                .is_allowed()
        );
        assert!(!authorize_void(Role::Cashier, bdt(1), &strict).is_allowed());
    }

    #[test]
    fn roles_order_from_least_to_most_trusted() {
        // Used to compare an approver against a required minimum.
        assert!(Role::Cashier < Role::Manager);
        assert!(Role::Manager < Role::Owner);
    }
}
