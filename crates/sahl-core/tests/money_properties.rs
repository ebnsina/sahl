//! Property tests for the money primitives.
//!
//! The unit tests next to the code prove specific cases; these prove the *laws*. Every invariant
//! here is one the VAT engine, invoice totals, and shift reconciliation assume without rechecking —
//! so if one of them can be broken, it will eventually be broken by a real basket at a real till.
//!
//! Counterexamples found here are written to `proptest-regressions/`, which is committed. A seed
//! that once broke the money math stays in the suite forever.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use proptest::prelude::*;
use sahl_core::{Currency, Money, Rate, Rounding};

const BDT: Currency = Currency::Bdt;

/// Weights small enough that the `magnitude × weight` intermediate stays well inside `u128`, and
/// counts matching realistic use: splitting a bill, apportioning a discount over line items.
fn weights() -> impl Strategy<Value = Vec<u64>> {
    prop::collection::vec(1u64..=10_000, 1..=24)
}

proptest! {
    /// The central guarantee. Everything downstream leans on it.
    #[test]
    fn allocation_always_sums_to_the_original(minor: i64, weights in weights()) {
        let original = Money::from_minor(minor, BDT);
        let parts = original.allocate(&weights).expect("valid weights");

        prop_assert_eq!(parts.len(), weights.len());
        prop_assert_eq!(Money::try_sum(parts, BDT).expect("no overflow"), original);
    }

    /// Splitting is allocation with equal weights, and inherits the same guarantee.
    #[test]
    fn splitting_always_sums_to_the_original(minor: i64, parts in 1usize..=64) {
        let original = Money::from_minor(minor, BDT);
        let split = original.split(parts).expect("positive part count");

        prop_assert_eq!(split.len(), parts);
        prop_assert_eq!(Money::try_sum(split, BDT).expect("no overflow"), original);
    }

    /// An even split must actually be even: no part may differ from another by more than one
    /// minor unit. Without this, "split three ways" could legally return 10.00 / 0.00 / 0.00.
    #[test]
    fn an_even_split_is_within_one_minor_unit(minor in -1_000_000_000i64..=1_000_000_000, parts in 1usize..=64) {
        let split = Money::from_minor(minor, BDT).split(parts).expect("positive part count");
        let largest = split.iter().map(|part| part.minor()).max().expect("non-empty");
        let smallest = split.iter().map(|part| part.minor()).min().expect("non-empty");

        prop_assert!(largest - smallest <= 1, "spread of {} across {parts} parts", largest - smallest);
    }

    /// A refund must mirror its sale line for line. If allocation were not sign-symmetric,
    /// returning a split bill would leak a minor unit somewhere.
    #[test]
    fn allocation_is_sign_symmetric(
        minor in (i64::MIN + 1)..=i64::MAX,
        weights in weights(),
    ) {
        let sale = Money::from_minor(minor, BDT).allocate(&weights).expect("valid weights");
        let refund = Money::from_minor(-minor, BDT).allocate(&weights).expect("valid weights");

        for (sold, refunded) in sale.iter().zip(refund.iter()) {
            prop_assert_eq!(sold.checked_neg().expect("negatable"), *refunded);
        }
    }

    /// Allocation must be a function of its inputs alone — no iteration-order or hash-order
    /// dependence. The terminal and the server both run this and must agree byte for byte.
    #[test]
    fn allocation_is_deterministic(minor: i64, weights in weights()) {
        let first = Money::from_minor(minor, BDT).allocate(&weights).expect("valid weights");
        let second = Money::from_minor(minor, BDT).allocate(&weights).expect("valid weights");

        prop_assert_eq!(first, second);
    }

    /// A larger weight never receives a smaller share. Obvious, and precisely the kind of thing
    /// largest-remainder distribution can break at the tie boundary if implemented carelessly.
    #[test]
    fn a_larger_weight_never_gets_less(
        minor in 0i64..=1_000_000_000,
        weights in weights(),
    ) {
        let parts = Money::from_minor(minor, BDT).allocate(&weights).expect("valid weights");

        for (i, wi) in weights.iter().enumerate() {
            for (j, wj) in weights.iter().enumerate() {
                if wi > wj {
                    prop_assert!(
                        parts[i].minor() >= parts[j].minor(),
                        "weight {wi} at {i} got {} but weight {wj} at {j} got {}",
                        parts[i].minor(), parts[j].minor()
                    );
                }
            }
        }
    }

    /// Subtracting what you added returns you exactly where you started. No drift.
    #[test]
    fn addition_and_subtraction_are_exact_inverses(
        a in (i64::MIN / 2)..=(i64::MAX / 2),
        b in (i64::MIN / 2)..=(i64::MAX / 2),
    ) {
        let start = Money::from_minor(a, BDT);
        let delta = Money::from_minor(b, BDT);
        let round_trip = start
            .checked_add(delta).expect("no overflow")
            .checked_sub(delta).expect("no overflow");

        prop_assert_eq!(round_trip, start);
    }

    /// Scaling by n/n is the identity for every rounding mode — a ratio that changes nothing must
    /// change nothing, including at the rounding boundary.
    #[test]
    fn scaling_by_unity_is_the_identity(minor: i64, factor in 1i64..=100_000) {
        for rounding in [Rounding::HalfUp, Rounding::HalfEven, Rounding::TowardZero] {
            let amount = Money::from_minor(minor, BDT);
            prop_assert_eq!(amount.mul_ratio(factor, factor, rounding).expect("no overflow"), amount);
        }
    }

    /// A zero rate contributes nothing, whatever the amount. Exempt and zero-rated supplies both
    /// land here, and both must produce exactly zero rather than a rounding artefact.
    #[test]
    fn a_zero_rate_yields_zero(minor: i64) {
        let tax = Money::from_minor(minor, BDT)
            .apply_rate(Rate::ZERO, Rounding::HalfUp)
            .expect("no overflow");

        prop_assert!(tax.is_zero());
    }

    /// Tax is never more than the amount it is charged on, for any rate up to 100%. A basket
    /// whose VAT exceeds its own value is the kind of bug that reaches a customer's receipt.
    #[test]
    fn tax_never_exceeds_its_base(
        minor in 0i64..=1_000_000_000_000,
        basis_points in 0i32..=10_000,
    ) {
        let base = Money::from_minor(minor, BDT);
        let tax = base
            .apply_rate(Rate::from_basis_points(basis_points), Rounding::HalfUp)
            .expect("no overflow");

        prop_assert!(tax.minor() <= base.minor(), "{tax} exceeds base {base}");
        prop_assert!(!tax.is_negative());
    }

    /// Rounding half-up can move a value by at most one minor unit away from truncation.
    /// This bounds the error of every proportional operation in the system.
    #[test]
    fn rounding_moves_by_at_most_one_minor_unit(
        minor in -1_000_000_000i64..=1_000_000_000,
        basis_points in 0i32..=10_000,
    ) {
        let base = Money::from_minor(minor, BDT);
        let rate = Rate::from_basis_points(basis_points);
        let rounded = base.apply_rate(rate, Rounding::HalfUp).expect("no overflow");
        let truncated = base.apply_rate(rate, Rounding::TowardZero).expect("no overflow");

        prop_assert!((rounded.minor() - truncated.minor()).abs() <= 1);
    }

    /// Currency is part of a value's identity and is never silently coerced.
    #[test]
    fn different_currencies_never_combine(a: i64, b: i64) {
        let taka = Money::from_minor(a, Currency::Bdt);
        let riyal = Money::from_minor(b, Currency::Sar);

        prop_assert!(taka.checked_add(riyal).is_err());
        prop_assert!(taka.checked_sub(riyal).is_err());
    }
}
