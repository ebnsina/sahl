//! A scale label must come back as the number the scale printed, or be refused.
//!
//! The failure these guard is quiet. A label that parses to the wrong weight charges the wrong
//! amount and nothing anywhere says so — the scanner beeped, the line appeared, the customer paid.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use proptest::prelude::*;
use sahl_core::money::{Currency, Money};
use sahl_core::quantity::Quantity;
use sahl_core::scale::{Embedded, ScaleError, ScaleFormat, ScannedValue};

const BDT: Currency = Currency::Bdt;

/// Build the label a scale would print, check digit and all.
fn label(prefix: &str, item: &str, value: &str) -> String {
    let twelve = format!("{prefix}{item}{value}");
    let mut sum: u32 = 0;
    for (index, character) in twelve.chars().enumerate() {
        let digit = character.to_digit(10).unwrap();
        sum += digit * if index % 2 == 0 { 1 } else { 3 };
    }
    format!("{twelve}{}", (10 - sum % 10) % 10)
}

proptest! {
    #[test]
    fn any_weight_the_scale_prints_comes_back_unchanged(
        item in 0_u32..99_999,
        grams in 0_u32..99_999,
    ) {
        let format = ScaleFormat::new("20", 5, Embedded::Weight, 5, 3, 0).expect("valid");
        let barcode = label("20", &format!("{item:05}"), &format!("{grams:05}"));
        let scan = format.parse(&barcode, BDT).expect("parses");

        prop_assert_eq!(scan.item_code, format!("{item:05}"));
        prop_assert_eq!(scan.value, ScannedValue::Weight(Quantity::from_milli(i64::from(grams))));
    }

    #[test]
    fn any_price_the_scale_prints_comes_back_unchanged(
        item in 0_u32..99_999,
        minor in 0_u32..99_999,
    ) {
        let format = ScaleFormat::new("21", 5, Embedded::Price, 5, 2, 0).expect("valid");
        let barcode = label("21", &format!("{item:05}"), &format!("{minor:05}"));
        let scan = format.parse(&barcode, BDT).expect("parses");

        prop_assert_eq!(scan.value, ScannedValue::Price(Money::from_minor(i64::from(minor), BDT)));
    }

    #[test]
    fn changing_any_single_digit_is_caught(
        item in 0_u32..99_999,
        grams in 0_u32..99_999,
        position in 0_usize..13,
        shift in 1_u32..10,
    ) {
        // A scanner misreading one digit is the realistic corruption, and it is exactly what the
        // check digit exists to catch. Every position, every wrong value.
        let format = ScaleFormat::new("20", 5, Embedded::Weight, 5, 3, 0).expect("valid");
        let good = label("20", &format!("{item:05}"), &format!("{grams:05}"));

        let mut digits: Vec<char> = good.chars().collect();
        let original = digits[position].to_digit(10).unwrap();
        digits[position] = char::from_digit((original + shift) % 10, 10).unwrap();
        let corrupt: String = digits.into_iter().collect();

        prop_assume!(corrupt != good);
        prop_assert!(
            format.parse(&corrupt, BDT).is_err(),
            "{} passed as a valid label", corrupt
        );
    }

    #[test]
    fn an_ordinary_supplier_barcode_is_never_read_as_a_weight(
        body in 100_000_000_000_u64..999_999_999_999,
    ) {
        // Prefix 89 is India, 88 is Bangladesh's neighbours — none of them are ours. A supplier
        // code read as a scale label would attach a nonsense weight to a real product.
        let format = ScaleFormat::new("20", 5, Embedded::Weight, 5, 3, 0).expect("valid");
        let twelve = format!("{body:012}");
        prop_assume!(!twelve.starts_with("20"));
        let barcode = label(&twelve[..2], &twelve[2..7], &twelve[7..12]);

        let refused = matches!(
            format.parse(&barcode, BDT),
            Err(ScaleError::NotAScaleLabel { .. })
        );
        prop_assert!(refused);
        prop_assert!(!format.matches(&barcode));
    }

    #[test]
    fn a_valid_label_is_never_rejected_by_matches(
        item in 0_u32..99_999,
        grams in 0_u32..99_999,
    ) {
        // `matches` decides whether the scan even reaches `parse`. If it disagrees, a good label is
        // dropped into the ordinary barcode lookup and the product is simply not found.
        let format = ScaleFormat::new("20", 5, Embedded::Weight, 5, 3, 0).expect("valid");
        let barcode = label("20", &format!("{item:05}"), &format!("{grams:05}"));

        prop_assert!(format.matches(&barcode));
        prop_assert!(format.parse(&barcode, BDT).is_ok());
    }
}
