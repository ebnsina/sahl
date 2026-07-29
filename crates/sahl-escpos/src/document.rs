//! Assembling a printable receipt.

use sahl_core::{Money, Quantity};

use crate::command::{self, Align, DrawerPin};
use crate::receipt::{PaperWidth, amount, columns, rule};

/// One printed line item.
#[derive(Debug, Clone)]
pub struct ReceiptLine {
    pub name: String,
    pub quantity: Quantity,
    pub unit_price: Money,
    pub total: Money,
    /// Printed and marked, never omitted — paper must match the log.
    pub voided: bool,
}

/// A VAT summary row.
#[derive(Debug, Clone)]
pub struct ReceiptTaxGroup {
    pub label: String,
    pub taxable_base: Money,
    pub tax: Money,
}

/// Everything a receipt prints. Amounts arrive exact; this module only places characters.
#[derive(Debug, Clone)]
pub struct ReceiptData {
    pub shop_name: String,
    pub shop_address: Option<String>,
    /// VAT registration — Mushak requires it on the face of a tax invoice, and ZATCA likewise.
    pub tax_registration: Option<String>,
    /// Human-readable invoice number.
    pub invoice_number: String,
    /// Pre-formatted by the caller with `Intl` in the outlet's timezone, because a receipt shows
    /// local time and only the caller knows the outlet.
    pub printed_at: String,
    pub currency_label: String,
    pub lines: Vec<ReceiptLine>,
    pub tax_groups: Vec<ReceiptTaxGroup>,
    pub discount: Option<Money>,
    pub net: Money,
    pub tax: Money,
    pub total: Money,
    pub tenders: Vec<(String, Money)>,
    pub change: Option<Money>,
    pub footer: Option<String>,
}

/// A rendered receipt, ready to write to a device.
#[derive(Debug, Clone)]
pub struct Document {
    bytes: Vec<u8>,
}

impl Document {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Render a receipt. `open_drawer` appends the pulse so the drawer opens as the paper prints.
    #[must_use]
    pub fn render(data: &ReceiptData, paper: PaperWidth, open_drawer: bool) -> Self {
        let width = paper.columns();
        let mut out = command::initialize();

        // A previous job that died mid-print can leave a printer in an odd code page; setting it
        // explicitly costs three bytes and removes a whole class of "why is it printing boxes".
        out.extend(command::code_page(command::CODE_PAGE_CP437));

        // Header
        out.extend(command::align(Align::Center));
        out.extend(command::emphasis(true));
        out.extend(command::size(true, true));
        out.extend(line(&data.shop_name));
        out.extend(command::size(false, false));
        out.extend(command::emphasis(false));

        if let Some(address) = &data.shop_address {
            out.extend(line(address));
        }
        if let Some(registration) = &data.tax_registration {
            out.extend(line(&format!("VAT Reg: {registration}")));
        }

        out.extend(command::align(Align::Left));
        out.extend(line(&rule(width, '-')));

        // Separate rows: sharing one truncates the invoice number on 58mm, and both Mushak 6.3
        // and ZATCA require it in full.
        out.extend(line(&format!("Invoice: {}", data.invoice_number)));
        out.extend(line(&format!("Date:    {}", data.printed_at)));
        out.extend(line(&rule(width, '-')));

        // Two rows per item: 32 columns cannot fit a real product name beside three numbers.
        for item in &data.lines {
            if item.voided {
                out.extend(line(&format!("{} (VOID)", item.name)));
                out.extend(line(&columns(
                    &format!(
                        "  {} x {}",
                        quantity(item.quantity),
                        amount(item.unit_price)
                    ),
                    "0.00",
                    width,
                )));
            } else {
                out.extend(line(&item.name));
                out.extend(line(&columns(
                    &format!(
                        "  {} x {}",
                        quantity(item.quantity),
                        amount(item.unit_price)
                    ),
                    &amount(item.total),
                    width,
                )));
            }
        }

        out.extend(line(&rule(width, '-')));

        // Totals
        if let Some(discount) = data.discount.filter(|value| !value.is_zero()) {
            out.extend(line(&columns(
                "Discount",
                &format!("-{}", amount(discount)),
                width,
            )));
        }
        out.extend(line(&columns("Subtotal", &amount(data.net), width)));

        for group in &data.tax_groups {
            out.extend(line(&columns(&group.label, &amount(group.tax), width)));
        }

        out.extend(command::emphasis(true));
        out.extend(line(&columns(
            &format!("TOTAL {}", data.currency_label),
            &amount(data.total),
            width,
        )));
        out.extend(command::emphasis(false));

        for (method, value) in &data.tenders {
            out.extend(line(&columns(method, &amount(*value), width)));
        }
        if let Some(change) = data.change.filter(|value| !value.is_zero()) {
            out.extend(line(&columns("Change", &amount(change), width)));
        }

        // Footer
        out.extend(line(&rule(width, '-')));
        out.extend(command::align(Align::Center));
        if let Some(footer) = &data.footer {
            out.extend(line(footer));
        }

        // Feed first, or the last lines are still inside the mechanism when the blade fires.
        out.extend(command::feed(3));
        out.extend(command::cut());

        if open_drawer {
            out.extend(command::open_drawer(DrawerPin::Two));
        }

        Self { bytes: out }
    }

    /// Drawer pulse alone, for a no-sale open — itself an audited event.
    #[must_use]
    pub fn drawer_only() -> Self {
        Self {
            bytes: command::open_drawer(DrawerPin::Two),
        }
    }
}

/// A line of text plus a newline.
///
/// Non-ASCII becomes '?': visible and diagnosable, unlike CP437's garbage boxes.
fn line(text: &str) -> Vec<u8> {
    let mut out: Vec<u8> = text
        .chars()
        .map(|character| {
            if character.is_ascii() {
                character as u8
            } else {
                b'?'
            }
        })
        .collect();
    out.push(b'\n');
    out
}

/// Trailing zeros trimmed, so whole units read as "2" not "2.000".
fn quantity(value: Quantity) -> String {
    let milli = value.milli();
    if milli % Quantity::MILLI_PER_UNIT == 0 {
        (milli / Quantity::MILLI_PER_UNIT).to_string()
    } else {
        let text = format!(
            "{}.{:03}",
            milli / Quantity::MILLI_PER_UNIT,
            (milli % Quantity::MILLI_PER_UNIT).unsigned_abs()
        );
        text.trim_end_matches('0').to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sahl_core::Currency;

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, Currency::Bdt)
    }

    fn sample() -> ReceiptData {
        ReceiptData {
            shop_name: "Karim Store".to_owned(),
            shop_address: Some("Dhanmondi, Dhaka".to_owned()),
            tax_registration: Some("001234567-0101".to_owned()),
            invoice_number: "A-000142".to_owned(),
            printed_at: "29 Jul 2026, 2:26 PM".to_owned(),
            currency_label: "BDT".to_owned(),
            lines: vec![
                ReceiptLine {
                    name: "Basmati rice 5kg".to_owned(),
                    quantity: Quantity::ONE,
                    unit_price: bdt(48_000),
                    total: bdt(48_000),
                    voided: false,
                },
                ReceiptLine {
                    name: "Rice loose".to_owned(),
                    quantity: Quantity::from_milli(1_234),
                    unit_price: bdt(8_000),
                    total: bdt(9_872),
                    voided: false,
                },
            ],
            tax_groups: vec![ReceiptTaxGroup {
                label: "VAT 15%".to_owned(),
                taxable_base: bdt(50_324),
                tax: bdt(7_548),
            }],
            discount: None,
            net: bdt(50_324),
            tax: bdt(7_548),
            total: bdt(57_872),
            tenders: vec![("Cash".to_owned(), bdt(60_000))],
            change: Some(bdt(2_128)),
            footer: Some("Thank you".to_owned()),
        }
    }

    fn text_of(document: &Document) -> String {
        String::from_utf8_lossy(document.bytes()).to_string()
    }

    #[test]
    fn a_receipt_starts_with_a_reset() {
        let document = Document::render(&sample(), PaperWidth::Mm58, false);
        assert_eq!(&document.bytes()[..2], &[0x1B, b'@']);
    }

    #[test]
    fn a_receipt_ends_with_a_cut() {
        let document = Document::render(&sample(), PaperWidth::Mm58, false);
        let bytes = document.bytes();
        assert_eq!(&bytes[bytes.len() - 4..], &[0x1D, b'V', 66, 0x00]);
    }

    #[test]
    fn the_drawer_pulse_follows_the_cut_when_cash_was_taken() {
        let document = Document::render(&sample(), PaperWidth::Mm58, true);
        let bytes = document.bytes();
        assert_eq!(&bytes[bytes.len() - 5..], &[0x1B, b'p', 0, 25, 25]);
    }

    #[test]
    fn no_drawer_pulse_when_no_cash_moved() {
        let document = Document::render(&sample(), PaperWidth::Mm58, false);
        let bytes = document.bytes();
        assert!(
            !bytes.windows(2).any(|pair| pair == [0x1B, b'p']),
            "a card-only sale must not open the drawer"
        );
    }

    #[test]
    fn every_text_line_fits_the_paper_width() {
        // The whole point of the column arithmetic. A line that overflows wraps, and a wrapped
        // price is a price the customer cannot find.
        for paper in [PaperWidth::Mm58, PaperWidth::Mm80] {
            let document = Document::render(&sample(), paper, false);
            let printable: String = text_of(&document)
                .chars()
                .filter(|c| *c == '\n' || !c.is_control())
                .collect();

            for row in printable.lines() {
                // Strip the control-sequence remnants that survive the filter.
                let visible = row
                    .trim_start_matches(['a', 'E', '!', 't', '@', 'd', '\u{0}', '\u{1}', '\u{2}']);
                assert!(
                    visible.chars().count() <= paper.columns(),
                    "{:?}: {visible:?} is {} columns, limit {}",
                    paper,
                    visible.chars().count(),
                    paper.columns()
                );
            }
        }
    }

    #[test]
    fn totals_and_amounts_appear_on_the_receipt() {
        let document = Document::render(&sample(), PaperWidth::Mm58, true);
        let text = text_of(&document);

        assert!(text.contains("Karim Store"));
        assert!(text.contains("A-000142"));
        assert!(text.contains("578.72"), "the total");
        assert!(text.contains("21.28"), "the change");
        assert!(text.contains("VAT Reg: 001234567-0101"));
    }

    #[test]
    fn a_weighed_quantity_prints_readably() {
        let document = Document::render(&sample(), PaperWidth::Mm58, false);
        let text = text_of(&document);
        assert!(text.contains("1.234 x 80.00"), "weighed line");
        assert!(text.contains("1 x 480.00"), "whole-unit line trims to '1'");
    }

    #[test]
    fn a_voided_line_is_printed_not_omitted() {
        // A receipt that silently drops a line gives the customer nothing to query, and makes paper
        // disagree with the log.
        let mut data = sample();
        data.lines[1].voided = true;
        let text = text_of(&Document::render(&data, PaperWidth::Mm58, false));

        assert!(text.contains("Rice loose (VOID)"));
    }

    #[test]
    fn non_ascii_degrades_visibly_rather_than_into_garbage() {
        // Until the raster path is wired up, Bangla and Arabic cannot print as CP437 characters.
        // Visible '?' is diagnosable; a wall of random boxes is not.
        let mut data = sample();
        data.lines[0].name = "চাল ৫ কেজি".to_owned();
        let text = text_of(&Document::render(&data, PaperWidth::Mm58, false));

        assert!(text.contains('?'));
        assert!(!text.contains('চ'));
    }

    #[test]
    fn the_wider_paper_uses_the_extra_columns() {
        let narrow = Document::render(&sample(), PaperWidth::Mm58, false);
        let wide = Document::render(&sample(), PaperWidth::Mm80, false);
        assert!(wide.bytes().len() > narrow.bytes().len());
    }
}
