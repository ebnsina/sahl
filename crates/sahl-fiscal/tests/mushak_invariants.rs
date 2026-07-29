//! Properties a Mushak 6.3 must hold for any basket.
//!
//! These are the checks an inspector performs with a calculator: add column 6, add column 9, see
//! whether they make column 10, and see whether the total row matches. An example test proves one
//! basket; these prove the arithmetic cannot be made to disagree.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use proptest::prelude::*;
use sahl_core::Timestamp;
use sahl_core::money::{Currency, Money};
use sahl_core::quantity::Quantity;
use sahl_core::tax::{Discount, LineInput, OrderInput, TaxClass, calculate};
use sahl_fiscal::bd_mushak::{Mushak63, build};
use sahl_fiscal::{Buyer, FiscalLine, Invoice, Seller};
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;

/// Rates a Bangladeshi merchant actually charges, plus the two nil treatments.
fn tax_class() -> impl Strategy<Value = TaxClass> {
    prop_oneof![
        Just(TaxClass::standard(1500)),
        Just(TaxClass::standard(1000)),
        Just(TaxClass::standard(750)),
        Just(TaxClass::standard(500)),
        Just(TaxClass::ZeroRated),
        Just(TaxClass::Exempt),
    ]
}

/// Bounded to stay under the Rule 40(1) threshold, so no basket demands a named buyer.
fn basket() -> impl Strategy<Value = Vec<(i64, i64, TaxClass)>> {
    // Prices up to Tk 200, quantities up to 5 units — a basket that stays under Rule 40(1).
    prop::collection::vec((1_i64..20_000, 1_i64..5_000, tax_class()), 1..8)
}

fn challan_for(basket: &[(i64, i64, TaxClass)]) -> Mushak63 {
    let lines: Vec<LineInput> = basket
        .iter()
        .map(|(price, milli, class)| LineInput {
            unit_price: Money::from_minor(*price, BDT),
            quantity: Quantity::from_milli(*milli),
            tax_class: *class,
            discount: Discount::None,
        })
        .collect();

    let totals = calculate(&OrderInput::new(BDT, lines)).expect("calculates");

    let described: Vec<FiscalLine> = basket
        .iter()
        .enumerate()
        .map(|(index, (_, milli, _))| FiscalLine {
            description: format!("Item {index}"),
            unit: "kg".to_owned(),
            quantity_milli: *milli,
        })
        .collect();

    build(&Invoice {
        sale_id: Uuid::from_u128(1),
        sequence: 1,
        issued_at: Timestamp::from_millis(1_753_000_000_000),
        seller: Seller {
            name: "Karim Store".to_owned(),
            registration: "0031234567890".to_owned(),
            address: "12 Dhanmondi 27, Dhaka".to_owned(),
        },
        buyer: Buyer::default(),
        lines: described,
        totals,
        destination: None,
    })
    .expect("builds")
}

proptest! {
    #[test]
    fn every_line_satisfies_column_six_plus_nine_equals_ten(basket in basket()) {
        let challan = challan_for(&basket);
        for line in &challan.lines {
            prop_assert_eq!(
                line.total_value.checked_add(line.vat_amount).unwrap(),
                line.total_with_tax,
                "line {} broke 6 + 9 = 10", line.serial
            );
        }
    }

    #[test]
    fn the_total_row_is_the_exact_sum_of_the_lines(basket in basket()) {
        let challan = challan_for(&basket);

        let net: i64 = challan.lines.iter().map(|line| line.total_value.minor()).sum();
        let vat: i64 = challan.lines.iter().map(|line| line.vat_amount.minor()).sum();
        let gross: i64 = challan.lines.iter().map(|line| line.total_with_tax.minor()).sum();

        prop_assert_eq!(challan.total_value.minor(), net);
        prop_assert_eq!(challan.total_vat.minor(), vat);
        prop_assert_eq!(challan.total_with_tax.minor(), gross);
    }

    #[test]
    fn serials_are_one_based_and_contiguous(basket in basket()) {
        let challan = challan_for(&basket);
        for (index, line) in challan.lines.iter().enumerate() {
            prop_assert_eq!(usize::try_from(line.serial).unwrap(), index + 1);
        }
    }

    #[test]
    fn a_nil_rated_line_carries_no_vat(basket in basket()) {
        // Zero-rated and exempt both show rate zero. If either produced tax, the challan would be
        // claiming VAT the merchant never charged.
        let challan = challan_for(&basket);
        for line in &challan.lines {
            if line.vat_rate_basis_points == 0 {
                prop_assert_eq!(line.vat_amount.minor(), 0);
                prop_assert_eq!(line.total_value, line.total_with_tax);
            }
        }
    }

    #[test]
    fn column_six_never_exceeds_column_ten(basket in basket()) {
        // Net above gross would mean negative tax — the shape of an inclusive/exclusive mix-up.
        let challan = challan_for(&basket);
        for line in &challan.lines {
            prop_assert!(line.total_value.minor() <= line.total_with_tax.minor());
        }
    }
}
