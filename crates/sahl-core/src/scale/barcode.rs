//! Reading a weight or a price out of an EAN-13 printed by a counter scale.
//!
//! GS1 reserves prefixes 02 and 20–29 for restricted circulation — codes that mean something only
//! inside one shop. Scale vendors use them for exactly this, but *what* the digits mean varies by
//! vendor and by how the shop configured the scale. So the layout is a setting, never a guess: read
//! a price-embedded label as a weight and the till charges 1.2 kg of nothing.

use serde::{Deserialize, Serialize};

use crate::money::{Currency, Money};
use crate::quantity::Quantity;

use super::ScaleError;

/// What the value digits on a label mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Embedded {
    /// The scale weighed it; the till still works out what that costs.
    Weight,
    /// The scale already priced it. **The till must not recompute** — the label in the customer's
    /// hand is the number they agreed to, and a unit price edited since would silently disagree.
    Price,
}

/// How one outlet's scale lays out its labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaleFormat {
    /// The leading digits that mark a label as coming from the scale rather than a supplier.
    prefix: String,
    /// Digits of item code, which map to a product's barcode field.
    item_digits: u8,
    embedded: Embedded,
    value_digits: u8,
    /// Where the decimal point sits in the value. Five digits and three decimals is 12345 → 12.345.
    value_decimals: u8,
    /// Vendor digits between the value and the check digit — usually the scale's own check over the
    /// price field. Deliberately ignored: the weighting is vendor-specific and a wrong guess would
    /// reject good labels.
    filler_digits: u8,
}

/// A label is a full EAN-13.
const LENGTH: usize = 13;

impl ScaleFormat {
    /// # Errors
    /// [`ScaleError::BadFormat`] unless the fields plus one check digit come to exactly 13;
    /// [`ScaleError::BadPrefix`] for a prefix that is not 1–3 digits.
    pub fn new(
        prefix: &str,
        item_digits: u8,
        embedded: Embedded,
        value_digits: u8,
        value_decimals: u8,
        filler_digits: u8,
    ) -> Result<Self, ScaleError> {
        if prefix.is_empty() || prefix.len() > 3 || !prefix.chars().all(|c| c.is_ascii_digit()) {
            return Err(ScaleError::BadPrefix {
                length: prefix.len(),
            });
        }

        let total = prefix
            .len()
            .saturating_add(usize::from(item_digits))
            .saturating_add(usize::from(value_digits))
            .saturating_add(usize::from(filler_digits))
            .saturating_add(1);
        if total != LENGTH || item_digits == 0 || value_digits == 0 {
            return Err(ScaleError::BadFormat {
                prefix: prefix.len(),
                item: item_digits,
                value: value_digits,
                filler: filler_digits,
                total,
            });
        }

        Ok(Self {
            prefix: prefix.to_owned(),
            item_digits,
            embedded,
            value_digits,
            value_decimals,
            filler_digits,
        })
    }

    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    #[must_use]
    pub const fn embedded(&self) -> Embedded {
        self.embedded
    }

    #[must_use]
    pub const fn item_digits(&self) -> u8 {
        self.item_digits
    }

    #[must_use]
    pub const fn value_digits(&self) -> u8 {
        self.value_digits
    }

    #[must_use]
    pub const fn value_decimals(&self) -> u8 {
        self.value_decimals
    }

    #[must_use]
    pub const fn filler_digits(&self) -> u8 {
        self.filler_digits
    }

    /// Whether this barcode is one of ours at all.
    ///
    /// Cheap enough to run on every scan, which is the point — an ordinary supplier barcode must
    /// fall through to the normal lookup untouched.
    #[must_use]
    pub fn matches(&self, barcode: &str) -> bool {
        barcode.len() == LENGTH
            && barcode.starts_with(&self.prefix)
            && barcode.chars().all(|c| c.is_ascii_digit())
    }

    /// Pull the item code and the embedded value out of a scanned label.
    ///
    /// # Errors
    /// [`ScaleError`] if the barcode is the wrong length, not digits, not ours, fails its check
    /// digit, or carries more precision than the target unit can hold.
    pub fn parse(&self, barcode: &str, currency: Currency) -> Result<ScaleScan, ScaleError> {
        if barcode.len() != LENGTH {
            return Err(ScaleError::WrongLength {
                length: barcode.len(),
            });
        }
        if let Some(found) = barcode.chars().find(|c| !c.is_ascii_digit()) {
            return Err(ScaleError::NotANumber { found });
        }
        if !barcode.starts_with(&self.prefix) {
            return Err(ScaleError::NotAScaleLabel {
                barcode: barcode.to_owned(),
                prefix: self.prefix.clone(),
            });
        }

        // A single misread digit is a wrong weight on a real bill, and the scanner beeped the same
        // either way. The check digit is the only thing standing between that and the customer.
        let expected = check_digit(barcode.get(..12).unwrap_or_default())?;
        let found = digit_at(barcode, 12)?;
        if found != expected {
            return Err(ScaleError::BadCheckDigit { found, expected });
        }

        let item_start = self.prefix.len();
        let item_end = item_start.saturating_add(usize::from(self.item_digits));
        let value_end = item_end.saturating_add(usize::from(self.value_digits));

        let item_code = barcode
            .get(item_start..item_end)
            .unwrap_or_default()
            .to_owned();
        let raw = barcode
            .get(item_end..value_end)
            .unwrap_or_default()
            .parse::<i64>()
            .map_err(|_| ScaleError::WrongLength { length: LENGTH })?;

        let value = match self.embedded {
            Embedded::Weight => ScannedValue::Weight(self.to_quantity(raw)?),
            Embedded::Price => ScannedValue::Price(self.to_money(raw, currency)?),
        };

        Ok(ScaleScan { item_code, value })
    }

    /// Scale up to thousandths, which is what `Quantity` holds.
    fn to_quantity(&self, raw: i64) -> Result<Quantity, ScaleError> {
        let spare = 3_u8
            .checked_sub(self.value_decimals)
            .ok_or(ScaleError::TooPrecise {
                decimals: self.value_decimals,
                unit: "thousandths",
            })?;
        let scale = 10_i64.pow(u32::from(spare));
        let milli = raw
            .checked_mul(scale)
            .ok_or(crate::money::MoneyError::Overflow)?;
        Ok(Quantity::from_milli(milli))
    }

    fn to_money(&self, raw: i64, currency: Currency) -> Result<Money, ScaleError> {
        let spare =
            currency
                .exponent()
                .checked_sub(self.value_decimals)
                .ok_or(ScaleError::TooPrecise {
                    decimals: self.value_decimals,
                    unit: "minor units",
                })?;
        let scale = 10_i64.pow(u32::from(spare));
        let minor = raw
            .checked_mul(scale)
            .ok_or(crate::money::MoneyError::Overflow)?;
        Ok(Money::from_minor(minor, currency))
    }
}

/// What a scanned label turned out to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaleScan {
    /// The digits identifying the product. Matched against the catalogue's barcodes, so the shop
    /// keeps one lookup rather than a second parallel table of scale codes.
    pub item_code: String,
    pub value: ScannedValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScannedValue {
    Weight(Quantity),
    /// Already priced by the scale. Sell it at this figure.
    Price(Money),
}

/// Standard EAN-13 modulo-10 over the first twelve digits.
fn check_digit(twelve: &str) -> Result<u32, ScaleError> {
    let mut sum: u32 = 0;
    for (index, character) in twelve.chars().enumerate() {
        let digit = character
            .to_digit(10)
            .ok_or(ScaleError::NotANumber { found: character })?;
        // Positions alternate 1×, 3×, starting at 1× for the leftmost digit.
        let weight = if index % 2 == 0 { 1 } else { 3 };
        sum = sum.saturating_add(digit.saturating_mul(weight));
    }
    Ok((10_u32.saturating_sub(sum % 10)) % 10)
}

fn digit_at(barcode: &str, index: usize) -> Result<u32, ScaleError> {
    barcode
        .chars()
        .nth(index)
        .and_then(|character| character.to_digit(10))
        .ok_or(ScaleError::WrongLength {
            length: barcode.len(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BDT: Currency = Currency::Bdt;

    /// The common weight layout: 2-digit prefix, 5-digit item, 5-digit weight in grams.
    fn weight_format() -> ScaleFormat {
        ScaleFormat::new("20", 5, Embedded::Weight, 5, 3, 0).expect("valid")
    }

    /// The common price layout: 2 + 5 item + 5 price + check.
    fn price_format() -> ScaleFormat {
        ScaleFormat::new("21", 5, Embedded::Price, 5, 2, 0).expect("valid")
    }

    /// Append the check digit the scanner would have produced.
    fn sealed(twelve: &str) -> String {
        let digit = check_digit(twelve).expect("digits");
        format!("{twelve}{digit}")
    }

    #[test]
    fn a_weight_label_yields_the_weight_the_scale_printed() {
        let barcode = sealed("201234501250");
        let scan = weight_format().parse(&barcode, BDT).expect("parses");

        assert_eq!(scan.item_code, "12345");
        assert_eq!(
            scan.value,
            ScannedValue::Weight(Quantity::from_milli(1_250))
        );
    }

    #[test]
    fn a_price_label_yields_money_the_till_must_not_recompute() {
        // The number on the label is what the customer agreed to at the counter. A unit price
        // edited since would silently disagree with the sticker in their hand.
        let barcode = sealed("211234500875");
        let scan = price_format().parse(&barcode, BDT).expect("parses");

        assert_eq!(scan.item_code, "12345");
        assert_eq!(scan.value, ScannedValue::Price(Money::from_minor(875, BDT)));
    }

    #[test]
    fn a_corrupt_scan_is_refused_rather_than_priced() {
        // A single misread digit is a wrong weight on a real bill, and the scanner beeped the same
        // either way.
        let mut barcode = sealed("201234501250");
        let last = barcode.pop().unwrap_or('0');
        let wrong = if last == '9' { '0' } else { '9' };
        barcode.push(wrong);

        assert!(matches!(
            weight_format().parse(&barcode, BDT),
            Err(ScaleError::BadCheckDigit { .. })
        ));
    }

    #[test]
    fn a_supplier_barcode_is_not_ours_and_falls_through() {
        let format = weight_format();
        assert!(!format.matches("8901234567895"));
        assert!(matches!(
            format.parse("8901234567895", BDT),
            Err(ScaleError::NotAScaleLabel { .. })
        ));
    }

    #[test]
    fn a_short_scan_is_refused() {
        assert!(matches!(
            weight_format().parse("2012345", BDT),
            Err(ScaleError::WrongLength { length: 7 })
        ));
    }

    #[test]
    fn letters_never_reach_the_parser() {
        assert!(matches!(
            weight_format().parse("20123450125X", BDT),
            Err(ScaleError::WrongLength { .. }) | Err(ScaleError::NotANumber { .. })
        ));
    }

    #[test]
    fn a_layout_that_does_not_add_up_to_thirteen_is_refused_at_configuration() {
        // Refused where an owner is looking at a settings screen, not at the counter mid-queue.
        assert!(matches!(
            ScaleFormat::new("20", 5, Embedded::Weight, 4, 3, 0),
            Err(ScaleError::BadFormat { total: 12, .. })
        ));
    }

    #[test]
    fn a_layout_with_no_item_code_is_refused() {
        assert!(ScaleFormat::new("20", 0, Embedded::Weight, 10, 3, 0).is_err());
    }

    #[test]
    fn a_prefix_must_be_short_digits() {
        assert!(matches!(
            ScaleFormat::new("", 5, Embedded::Weight, 7, 3, 0),
            Err(ScaleError::BadPrefix { .. })
        ));
        assert!(matches!(
            ScaleFormat::new("2A", 5, Embedded::Weight, 5, 3, 0),
            Err(ScaleError::BadPrefix { .. })
        ));
    }

    #[test]
    fn filler_digits_are_skipped_rather_than_read_as_value() {
        // Some scales put their own price check digit before the EAN check digit. Reading it as
        // part of the value would multiply the weight by ten.
        let format = ScaleFormat::new("20", 5, Embedded::Weight, 4, 3, 1).expect("valid");
        let barcode = sealed("201234512507");
        let scan = format.parse(&barcode, BDT).expect("parses");

        assert_eq!(
            scan.value,
            ScannedValue::Weight(Quantity::from_milli(1_250))
        );
    }

    #[test]
    fn a_label_in_kilos_rather_than_grams_still_lands_in_thousandths() {
        // Two decimals: 0125 is 1.25 kg, not 125 g.
        let format = ScaleFormat::new("20", 5, Embedded::Weight, 4, 2, 1).expect("valid");
        let barcode = sealed("201234501250");
        let scan = format.parse(&barcode, BDT).expect("parses");

        assert_eq!(
            scan.value,
            ScannedValue::Weight(Quantity::from_milli(1_250))
        );
    }

    #[test]
    fn more_precision_than_the_currency_holds_is_refused() {
        let format = ScaleFormat::new("21", 5, Embedded::Price, 5, 3, 0).expect("valid");
        let barcode = sealed("211234500875");

        assert!(matches!(
            format.parse(&barcode, BDT),
            Err(ScaleError::TooPrecise { .. })
        ));
    }

    #[test]
    fn a_zero_weight_label_parses_and_is_left_for_the_caller_to_refuse() {
        // Parsing and judging are separate: a zero-weight label is a real thing a jammed scale
        // prints, and the error belongs where it can name the product.
        let barcode = sealed("201234500000");
        let scan = weight_format().parse(&barcode, BDT).expect("parses");
        assert_eq!(scan.value, ScannedValue::Weight(Quantity::ZERO));
    }

    #[test]
    fn matches_is_cheap_and_lets_ordinary_barcodes_past() {
        let format = weight_format();
        assert!(format.matches(&sealed("201234501250")));
        assert!(!format.matches("21123450125"));
        assert!(!format.matches("2012345012XX0"));
    }

    #[test]
    fn the_check_digit_is_the_ean_13_one() {
        // Anchored against a published example so a rewrite of the loop cannot quietly redefine it.
        assert_eq!(check_digit("400638133393").expect("digits"), 1);
    }
}
