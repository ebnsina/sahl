//! Bangladesh: the Mushak 6.3 tax challan.
//!
//! Transcribed from the NBR form, which carries the citation
//! *[See Clauses (C) and (f) of Sub-Rule (1) of Rule 40]*. Ten numbered columns, and the numbers
//! matter — an inspector reads column 6, not "subtotal".
//!
//! The one thing worth knowing before reading further: **columns 5 and 6 are tax-exclusive.** The
//! form footnotes them "Value except all kinds of Tax". Bangladeshi retail prices tax-inclusive, so
//! a shelf label of ৳115 at 15% VAT appears here as 100 in column 6 and 15 in column 9. Printing
//! the shelf price in column 6 would overstate the taxable base on every line, which is the error
//! the whole tax engine's inclusive/exclusive split exists to prevent.

use sahl_core::money::{Money, MoneyError};
use sahl_core::tax::TaxClass;
use serde::{Deserialize, Serialize};

use crate::{FiscalError, Fiscalization, Invoice};

/// Above this supply value, the buyer must be named with address and BIN.
///
/// Rule 40(1): a registered person shall issue a Mushak 6.3 stating the buyer's name, address and
/// BIN where the supply price exceeds Tk 25,000. Held in minor units like every other amount.
/// Tk 25,000 in paisa.
pub const BUYER_REQUIRED_ABOVE_MINOR: i64 = 2_500_000;

/// One row of the challan, by the form's own column numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mushak63Line {
    /// Column 1 — S.L No, one-based.
    pub serial: u32,
    /// Column 2 — Goods/Service Description (in cases with Brand Name).
    pub description: String,
    /// Column 3 — Unit of Supply.
    pub unit: String,
    /// Column 4 — Quantity, in thousandths.
    pub quantity_milli: i64,
    /// Column 5 — Unit Value, **excluding all tax**.
    pub unit_value: Money,
    /// Column 6 — Total Value, **excluding all tax**.
    pub total_value: Money,
    /// Column 7 — Amount of Supplementary Duty.
    pub supplementary_duty: Money,
    /// Column 8 — VAT rate, in basis points. Zero for zero-rated and exempt alike; column 9
    /// distinguishes them by being zero for both, and the exemption is a matter for the ledger.
    pub vat_rate_basis_points: i32,
    /// Column 9 — VAT amount.
    pub vat_amount: Money,
    /// Column 10 — Value including all duty and tax.
    pub total_with_tax: Money,
}

/// A Mushak 6.3 challan.
///
/// Every amount is an exact integer of minor units computed by `sahl-core`. Nothing here is a
/// formatted string: the printer and the screen decide how a number looks, and a document that
/// carried pre-formatted text could not be reprinted in another locale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mushak63 {
    /// "Name of Registered Person".
    pub seller_name: String,
    /// "BIN of Registered Person".
    pub seller_bin: String,
    /// "Challan Issuing Address".
    pub issuing_address: String,

    /// "Name of Purchaser". Required above [`BUYER_REQUIRED_ABOVE_MINOR`].
    pub buyer_name: Option<String>,
    /// "BIN of Purchaser".
    pub buyer_bin: Option<String>,
    pub buyer_address: Option<String>,
    /// "Destination of Supply".
    pub destination: Option<String>,

    /// "Invoice No" — the per-device fiscal counter, rendered.
    pub invoice_number: String,
    /// "Date of Issue" and "Time of Issue" both derive from this; the form splits them, the
    /// document does not, so a reprint in another timezone cannot disagree with itself.
    pub issued_at_millis: i64,

    pub lines: Vec<Mushak63Line>,

    /// The "Total" row: columns 6, 7, 9 and 10 summed.
    pub total_value: Money,
    pub total_supplementary_duty: Money,
    pub total_vat: Money,
    pub total_with_tax: Money,
}

impl Mushak63 {
    /// Whether Rule 40(1) requires the buyer to be named on this challan.
    #[must_use]
    pub const fn buyer_required(total_with_tax: Money) -> bool {
        total_with_tax.minor() > BUYER_REQUIRED_ABOVE_MINOR
    }
}

/// The Bangladeshi regime.
///
/// Stateless: the counter and the registration details arrive on the [`Invoice`], because a device
/// that held its own copy of a BIN is a device that keeps issuing invoices with the old one after
/// the merchant re-registers.
#[derive(Debug, Clone, Copy, Default)]
pub struct BdMushak;

impl Fiscalization for BdMushak {
    fn regime(&self) -> &'static str {
        "bd_mushak"
    }

    fn issue(&self, invoice: &Invoice) -> Result<crate::Document, FiscalError> {
        Ok(crate::Document::BdMushak63(Box::new(build(invoice)?)))
    }
}

/// Build the challan.
///
/// # Errors
/// [`FiscalError`] if registration details are missing, the sale is empty, the line descriptions do
/// not match the computed lines, or Rule 40(1) requires a buyer who was not named.
pub fn build(invoice: &Invoice) -> Result<Mushak63, FiscalError> {
    const DOC: &str = "Mushak 6.3";

    if invoice.totals.lines.is_empty() {
        return Err(FiscalError::Empty);
    }
    if invoice.lines.len() != invoice.totals.lines.len() {
        // A silent zip would drop or misalign lines, putting one product's description against
        // another's tax — the kind of error that is invisible on the paper and fatal in an audit.
        return Err(FiscalError::Invalid(format!(
            "{} line descriptions for {} calculated lines",
            invoice.lines.len(),
            invoice.totals.lines.len()
        )));
    }
    if invoice.seller.registration.trim().is_empty() {
        return Err(FiscalError::Missing {
            field: "BIN of Registered Person",
            document: DOC,
        });
    }
    if invoice.seller.name.trim().is_empty() {
        return Err(FiscalError::Missing {
            field: "Name of Registered Person",
            document: DOC,
        });
    }
    if invoice.seller.address.trim().is_empty() {
        return Err(FiscalError::Missing {
            field: "Challan Issuing Address",
            document: DOC,
        });
    }

    if Mushak63::buyer_required(invoice.totals.total) {
        // Refused rather than issued blank. An incomplete challan above the threshold is not a
        // lesser compliance problem than no challan — it is the same one, discovered later.
        if invoice
            .buyer
            .name
            .as_ref()
            .is_none_or(|n| n.trim().is_empty())
        {
            return Err(FiscalError::Missing {
                field: "Name of Purchaser",
                document: DOC,
            });
        }
        if invoice
            .buyer
            .registration
            .as_ref()
            .is_none_or(|bin| bin.trim().is_empty())
        {
            return Err(FiscalError::Missing {
                field: "BIN of Purchaser",
                document: DOC,
            });
        }
        if invoice
            .buyer
            .address
            .as_ref()
            .is_none_or(|address| address.trim().is_empty())
        {
            return Err(FiscalError::Missing {
                field: "Address of Purchaser",
                document: DOC,
            });
        }
    }

    let currency = invoice.totals.total.currency();
    let zero = Money::from_minor(0, currency);

    let mut lines = Vec::with_capacity(invoice.totals.lines.len());
    let mut total_value = zero;
    let mut total_vat = zero;
    let mut total_with_tax = zero;

    for (index, (computed, described)) in invoice
        .totals
        .lines
        .iter()
        .zip(invoice.lines.iter())
        .enumerate()
    {
        let serial = u32::try_from(index.checked_add(1).ok_or(MoneyError::Overflow)?)
            .map_err(|_| MoneyError::Overflow)?;

        lines.push(Mushak63Line {
            serial,
            description: described.description.clone(),
            unit: described.unit.clone(),
            quantity_milli: described.quantity_milli,
            // Column 5 is a per-unit figure, so it is derived from the line's net rather than from
            // the shelf price — the shelf price includes tax and the discount has already landed.
            unit_value: unit_value(computed.net, described.quantity_milli)?,
            total_value: computed.net,
            // Supplementary duty is a separate levy from VAT and Sahl does not model it yet, so it
            // is an honest zero rather than a number folded into VAT.
            supplementary_duty: zero,
            vat_rate_basis_points: rate_of(computed.tax_class),
            vat_amount: computed.tax,
            total_with_tax: computed.total,
        });

        total_value = total_value.checked_add(computed.net)?;
        total_vat = total_vat.checked_add(computed.tax)?;
        total_with_tax = total_with_tax.checked_add(computed.total)?;
    }

    Ok(Mushak63 {
        seller_name: invoice.seller.name.clone(),
        seller_bin: invoice.seller.registration.clone(),
        issuing_address: invoice.seller.address.clone(),
        buyer_name: invoice.buyer.name.clone(),
        buyer_bin: invoice.buyer.registration.clone(),
        buyer_address: invoice.buyer.address.clone(),
        destination: invoice.destination.clone(),
        invoice_number: invoice.sequence.to_string(),
        issued_at_millis: invoice.issued_at.millis(),
        lines,
        total_value,
        total_supplementary_duty: zero,
        total_vat,
        total_with_tax,
    })
}

/// Column 5 from column 6: the net value spread back over the quantity.
///
/// Rounded, and deliberately not used to reconstruct column 6 — the form's own arithmetic is
/// quantity × unit value, which on a weighed line cannot come back to the exact net. Column 6
/// carries the exact figure; column 5 is the derived one, so any rounding lands where it does no
/// damage.
fn unit_value(net: Money, quantity_milli: i64) -> Result<Money, MoneyError> {
    if quantity_milli == 0 {
        return Ok(Money::from_minor(0, net.currency()));
    }
    net.mul_ratio(
        sahl_core::quantity::Quantity::MILLI_PER_UNIT,
        quantity_milli,
        sahl_core::money::Rounding::HalfUp,
    )
}

/// Column 8. Zero-rated and exempt both show a zero rate; the distinction lives in the VAT return,
/// not on the challan.
const fn rate_of(class: TaxClass) -> i32 {
    match class {
        TaxClass::Standard { rate } => rate.basis_points(),
        TaxClass::ZeroRated | TaxClass::Exempt => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{invoice, seller};
    use crate::{Buyer, FiscalLine, Seller};
    use sahl_core::money::{Currency, Money};
    use sahl_core::quantity::Quantity;
    use sahl_core::tax::{Discount, LineInput, OrderInput, TaxClass, calculate};

    const BDT: Currency = Currency::Bdt;

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, BDT)
    }

    #[test]
    fn a_tax_inclusive_shelf_price_lands_net_in_column_six() {
        // The whole reason this module is careful: ৳115 at 15% is 100 taxable, not 115. Printing
        // 115 in column 6 would overstate the taxable base on every line of every challan.
        let challan = build(&invoice(1)).expect("builds");

        assert_eq!(challan.lines[0].total_value, bdt(10_000), "column 6, net");
        assert_eq!(challan.lines[0].vat_amount, bdt(1_500), "column 9");
        assert_eq!(challan.lines[0].total_with_tax, bdt(11_500), "column 10");
        assert_eq!(challan.lines[0].vat_rate_basis_points, 1500, "column 8");
    }

    #[test]
    fn the_total_row_is_the_exact_sum_of_its_columns() {
        // An inspector adds the column up. A summary that disagrees with its own lines is the
        // single most obvious thing to find on a challan.
        let challan = build(&multi_line()).expect("builds");

        let net: i64 = challan.lines.iter().map(|l| l.total_value.minor()).sum();
        let vat: i64 = challan.lines.iter().map(|l| l.vat_amount.minor()).sum();
        let gross: i64 = challan.lines.iter().map(|l| l.total_with_tax.minor()).sum();

        assert_eq!(challan.total_value.minor(), net);
        assert_eq!(challan.total_vat.minor(), vat);
        assert_eq!(challan.total_with_tax.minor(), gross);
        assert_eq!(
            challan.total_value.checked_add(challan.total_vat),
            Ok(challan.total_with_tax),
            "columns 6 + 9 = column 10"
        );
    }

    #[test]
    fn serial_numbers_are_one_based_and_contiguous() {
        // Column 1 is S.L No. A gap reads as a removed line.
        let challan = build(&multi_line()).expect("builds");
        let serials: Vec<u32> = challan.lines.iter().map(|line| line.serial).collect();
        assert_eq!(serials, vec![1, 2, 3]);
    }

    #[test]
    fn an_exempt_line_shows_a_zero_rate_and_no_vat() {
        let challan = build(&multi_line()).expect("builds");
        let exempt = challan
            .lines
            .iter()
            .find(|line| line.description == "Fresh milk 1L")
            .expect("present");

        assert_eq!(exempt.vat_rate_basis_points, 0);
        assert_eq!(exempt.vat_amount, bdt(0));
        assert_eq!(exempt.total_value, exempt.total_with_tax);
    }

    #[test]
    fn a_weighed_line_keeps_the_exact_net_in_column_six() {
        // Column 5 is derived and may round; column 6 must not, because the total row sums it.
        let totals = calculate(&OrderInput::new(
            BDT,
            vec![LineInput {
                unit_price: bdt(4_600),
                quantity: Quantity::from_milli(1_234),
                tax_class: TaxClass::standard(1500),
                discount: Discount::None,
            }],
        ))
        .expect("calculates");

        let mut sale = invoice(7);
        sale.lines = vec![FiscalLine {
            description: "Rice, loose".to_owned(),
            unit: "kg".to_owned(),
            quantity_milli: 1_234,
        }];
        sale.totals = totals.clone();

        let challan = build(&sale).expect("builds");
        assert_eq!(challan.lines[0].total_value, totals.lines[0].net);
        assert_eq!(challan.total_value, totals.net);
    }

    #[test]
    fn a_small_sale_needs_no_buyer() {
        // Rule 40(1) applies above Tk 25,000. Demanding a BIN for a loaf of bread would make the
        // till unusable, which is its own kind of non-compliance.
        assert!(!Mushak63::buyer_required(bdt(2_500_000)));
        assert!(build(&invoice(1)).is_ok());
    }

    #[test]
    fn a_large_sale_is_refused_without_the_buyer_rule_forty_demands() {
        // Refused rather than issued blank: an incomplete challan above the threshold is the same
        // compliance problem as no challan, only discovered later.
        assert!(Mushak63::buyer_required(bdt(2_500_001)));

        let mut sale = large_sale();
        sale.buyer = Buyer::default();

        assert_eq!(
            build(&sale),
            Err(FiscalError::Missing {
                field: "Name of Purchaser",
                document: "Mushak 6.3"
            })
        );
    }

    #[test]
    fn a_large_sale_with_a_named_buyer_is_issued() {
        let challan = build(&large_sale()).expect("builds");
        assert_eq!(challan.buyer_name.as_deref(), Some("Rahim Enterprise"));
        assert_eq!(challan.buyer_bin.as_deref(), Some("0039876543210"));
    }

    #[test]
    fn a_buyer_named_with_a_blank_bin_is_still_refused() {
        // Whitespace is not a BIN, and a form field someone tabbed through is exactly how this
        // fails in practice.
        let mut sale = large_sale();
        sale.buyer.registration = Some("   ".to_owned());

        assert_eq!(
            build(&sale),
            Err(FiscalError::Missing {
                field: "BIN of Purchaser",
                document: "Mushak 6.3"
            })
        );
    }

    #[test]
    fn a_seller_without_a_bin_cannot_issue_anything() {
        let mut sale = invoice(1);
        sale.seller = Seller {
            registration: String::new(),
            ..seller()
        };

        assert_eq!(
            build(&sale),
            Err(FiscalError::Missing {
                field: "BIN of Registered Person",
                document: "Mushak 6.3"
            })
        );
    }

    #[test]
    fn descriptions_that_do_not_match_the_calculated_lines_are_refused() {
        // A silent zip would put one product's name against another's tax — invisible on paper,
        // fatal in an audit.
        let mut sale = multi_line();
        sale.lines.pop();

        assert!(matches!(build(&sale), Err(FiscalError::Invalid(_))));
    }

    #[test]
    fn an_empty_sale_produces_no_challan() {
        let mut sale = invoice(1);
        sale.lines.clear();
        sale.totals.lines.clear();

        assert_eq!(build(&sale), Err(FiscalError::Empty));
    }

    #[test]
    fn the_invoice_number_is_the_device_counter() {
        // Both regimes need a per-device monotonic counter and neither accepts a gap.
        assert_eq!(
            build(&invoice(4471)).expect("builds").invoice_number,
            "4471"
        );
    }

    #[test]
    fn issuing_through_the_trait_yields_a_mushak() {
        let document = BdMushak.issue(&invoice(1)).expect("issues");
        assert!(matches!(document, crate::Document::BdMushak63(_)));
        assert_eq!(BdMushak.regime(), "bd_mushak");
    }

    /// Three lines at three different VAT treatments — the mixed basket a grocery actually rings.
    fn multi_line() -> Invoice {
        let totals = calculate(&OrderInput::new(
            BDT,
            vec![
                LineInput {
                    unit_price: bdt(11_500),
                    quantity: Quantity::ONE,
                    tax_class: TaxClass::standard(1500),
                    discount: Discount::None,
                },
                LineInput {
                    unit_price: bdt(5_500),
                    quantity: Quantity::from_milli(2_000),
                    tax_class: TaxClass::standard(750),
                    discount: Discount::None,
                },
                LineInput {
                    unit_price: bdt(9_000),
                    quantity: Quantity::ONE,
                    tax_class: TaxClass::Exempt,
                    discount: Discount::None,
                },
            ],
        ))
        .expect("calculates");

        let mut sale = invoice(2);
        sale.lines = vec![
            FiscalLine {
                description: "Basmati rice 5kg".to_owned(),
                unit: "pcs".to_owned(),
                quantity_milli: 1_000,
            },
            FiscalLine {
                description: "Bread".to_owned(),
                unit: "pcs".to_owned(),
                quantity_milli: 2_000,
            },
            FiscalLine {
                description: "Fresh milk 1L".to_owned(),
                unit: "pcs".to_owned(),
                quantity_milli: 1_000,
            },
        ];
        sale.totals = totals;
        sale
    }

    /// A sale above the Rule 40(1) threshold.
    fn large_sale() -> Invoice {
        let totals = calculate(&OrderInput::new(
            BDT,
            vec![LineInput {
                unit_price: bdt(3_000_000),
                quantity: Quantity::ONE,
                tax_class: TaxClass::standard(1500),
                discount: Discount::None,
            }],
        ))
        .expect("calculates");

        let mut sale = invoice(3);
        sale.lines = vec![FiscalLine {
            description: "Bulk rice, 50 sacks".to_owned(),
            unit: "sack".to_owned(),
            quantity_milli: 1_000,
        }];
        sale.totals = totals;
        sale.buyer = Buyer {
            name: Some("Rahim Enterprise".to_owned()),
            registration: Some("0039876543210".to_owned()),
            address: Some("44 Motijheel C/A, Dhaka".to_owned()),
        };
        sale
    }
}
