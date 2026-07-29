//! Staff: who may do what, and what they did.
//!
//! Two halves of one control. The role model decides in advance; the audit trail records after the
//! fact. Either alone is weak — permissions with no record can be shared, and a record nobody set
//! rules for is just noise.

pub mod audit;
pub mod pin;
pub mod role;

pub use audit::{AuditEntry, Severity, from_sales, from_shifts, ranked, self_approved, unapproved};
pub use pin::PinError;
pub use role::{
    ApprovalPolicy, Authorization, Permission, Role, authorize, authorize_discount,
    authorize_discount_rate, authorize_void,
};
