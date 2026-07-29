//! Print a receipt to the terminal as it would appear on paper.
//!
//! Control bytes are stripped and the paper edge is drawn, so a layout change can be eyeballed
//! without a printer. Run with: cargo run -p sahl-escpos --example preview
fn main() {
    use sahl_core::{Currency, Money, Quantity};
    use sahl_escpos::{Document, PaperWidth, ReceiptData, ReceiptLine, ReceiptTaxGroup};

    let bdt = |minor| Money::from_minor(minor, Currency::Bdt);

    let data = ReceiptData {
        shop_name: "Karim Store".into(),
        shop_address: Some("Dhanmondi, Dhaka".into()),
        tax_registration: Some("001234567-0101".into()),
        invoice_number: "A-000142".into(),
        printed_at: "29 Jul 2026, 2:26 PM".into(),
        currency_label: "BDT".into(),
        lines: vec![
            ReceiptLine {
                name: "Basmati rice 5kg".into(),
                quantity: Quantity::ONE,
                unit_price: bdt(48_000),
                total: bdt(48_000),
                voided: false,
            },
            ReceiptLine {
                name: "Rice loose".into(),
                quantity: Quantity::from_milli(1_234),
                unit_price: bdt(8_000),
                total: bdt(9_872),
                voided: false,
            },
            ReceiptLine {
                name: "Cooking oil 2L".into(),
                quantity: Quantity::from_milli(2_000),
                unit_price: bdt(34_000),
                total: bdt(68_000),
                voided: true,
            },
            ReceiptLine {
                name: "Fresh milk 1L".into(),
                quantity: Quantity::ONE,
                unit_price: bdt(9_000),
                total: bdt(9_000),
                voided: false,
            },
        ],
        tax_groups: vec![
            ReceiptTaxGroup {
                label: "VAT 15%".into(),
                taxable_base: bdt(50_324),
                tax: bdt(7_548),
            },
            ReceiptTaxGroup {
                label: "Exempt".into(),
                taxable_base: bdt(9_000),
                tax: bdt(0),
            },
        ],
        discount: Some(bdt(1_000)),
        net: bdt(59_324),
        tax: bdt(7_548),
        total: bdt(65_872),
        tenders: vec![("Cash".into(), bdt(70_000))],
        change: Some(bdt(4_128)),
        footer: Some("Thank you".into()),
    };

    for paper in [PaperWidth::Mm58, PaperWidth::Mm80] {
        let document = Document::render(&data, paper, true);
        let width = paper.columns();
        println!(
            "\n{:?}  ({width} columns, {} bytes)",
            paper,
            document.bytes().len()
        );
        println!("┌{}┐", "─".repeat(width));
        for row in strip_controls(document.bytes()).split('\n') {
            if row.trim().is_empty() {
                continue;
            }
            println!("│{:<width$}│", row, width = width);
        }
        println!("└{}┘", "─".repeat(width));
    }
}

/// Remove ESC/POS control sequences by parsing them, not by filtering characters.
///
/// Filtering by character is the obvious approach and it is wrong: it eats the leading letter of
/// any word starting with a byte that also appears as a command argument, so "Basmati" prints as
/// "smati". Each sequence has a known fixed length, so skipping exactly that many bytes is both
/// simpler and correct.
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "example code"
)]
fn strip_controls(bytes: &[u8]) -> String {
    const ESC: u8 = 0x1B;
    const GS: u8 = 0x1D;

    let mut out = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let skip = match (bytes[index], bytes.get(index + 1)) {
            (ESC, Some(b'@')) => 2,
            (ESC, Some(b'a' | b'E' | b't' | b'd' | b'!')) => 3,
            (ESC, Some(b'p')) => 5,
            (GS, Some(b'!')) => 3,
            (GS, Some(b'V')) => 4,
            _ => {
                out.push(bytes[index] as char);
                1
            }
        };
        index += skip;
    }
    out
}
