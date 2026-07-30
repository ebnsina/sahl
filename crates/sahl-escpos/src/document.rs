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
    /// The ZATCA QR payload, already base64. Absent under every other regime — a QR nobody's
    /// jurisdiction asks for is ink and paper spent on nothing.
    pub qr: Option<String>,
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

    /// Render a kitchen ticket.
    ///
    /// No drawer pulse and no prices. A prep station has no till, and an amount on a ticket is a
    /// number somebody will eventually read as a quantity.
    #[must_use]
    pub fn render_kitchen(data: &KitchenTicketData, paper: PaperWidth) -> Self {
        let width = paper.columns();
        let mut out = command::initialize();
        out.extend(command::code_page(command::CODE_PAGE_CP437));

        out.extend(command::align(Align::Center));
        out.extend(command::emphasis(true));
        out.extend(command::size(true, true));

        // A cancellation is marked before anything else on the ticket, because the whole meaning of
        // what follows inverts and a cook reading it late has already started cooking.
        if data.is_cancellation {
            out.extend(line("*** CANCEL ***"));
        }
        out.extend(line(&data.station));
        out.extend(command::size(false, false));

        if let Some(table) = &data.table_label {
            out.extend(command::size(true, true));
            out.extend(line(&format!("TABLE {table}")));
            out.extend(command::size(false, false));
        }

        out.extend(command::emphasis(false));
        out.extend(command::align(Align::Left));
        out.extend(line(&rule(width, '=')));

        let mut header = format!("Round {}", data.round);
        if let Some(covers) = data.covers {
            // ASCII only. The code page is CP437 and a multi-byte character sent to it prints as
            // whatever those bytes happen to mean there — see the note at the top of the crate.
            header.push_str(&format!("   {covers} covers"));
        }
        out.extend(line(&header));
        out.extend(line(&data.printed_at));
        out.extend(line(&rule(width, '=')));

        for item in &data.lines {
            out.extend(command::emphasis(true));
            out.extend(command::size(false, true));
            out.extend(line(&format!(
                "{} x {}",
                quantity_label(item.quantity),
                item.name
            )));
            out.extend(command::size(false, false));
            out.extend(command::emphasis(false));

            // Indented on their own lines rather than inline: an option appended to a dish name
            // wraps into the next item on a 32-column roll and reads as part of it.
            for modifier in &item.modifiers {
                out.extend(line(&format!("    - {modifier}")));
            }
        }

        out.extend(line(&rule(width, '=')));
        out.extend(command::feed(3));
        out.extend(command::cut());

        Self { bytes: out }
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

        // A ZATCA simplified invoice is not compliant without this, so it goes above the footer
        // where a torn-off receipt still carries it.
        if let Some(payload) = &data.qr
            && let Ok(bytes) = command::qr(payload.as_bytes(), 5)
        {
            out.extend(bytes);
            out.extend(line(""));
        }

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
/// A quantity as a cook reads it: "2", not "2.000".
fn quantity_label(quantity: Quantity) -> String {
    let milli = quantity.milli();
    if milli % Quantity::MILLI_PER_UNIT == 0 {
        return (milli / Quantity::MILLI_PER_UNIT).to_string();
    }
    let whole = milli / Quantity::MILLI_PER_UNIT;
    let fraction = (milli % Quantity::MILLI_PER_UNIT).abs();
    format!("{whole}.{fraction:03}")
        .trim_end_matches('0')
        .to_owned()
}

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
            qr: None,
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

/// One station's instruction, as it prints.
///
/// Deliberately not a receipt. A kitchen ticket carries **no prices** — a number beside an item is a
/// number somebody will eventually mistake for a quantity — and it is set large, because it is read
/// across a hot room at a glance rather than held and studied.
#[derive(Debug, Clone)]
pub struct KitchenTicketData {
    /// "KITCHEN", "BAR". Shouted, because it is the first thing read.
    pub station: String,
    /// True when this cancels rather than orders. The two must never be confused: a cancellation
    /// read as an order gets the dish made twice.
    pub is_cancellation: bool,
    pub table_label: Option<String>,
    pub covers: Option<u32>,
    /// Which round. A cook reading "2" knows the first is already out.
    pub round: u32,
    /// Pre-formatted by the caller, like every other time on a printed document.
    pub printed_at: String,
    pub lines: Vec<KitchenTicketLine>,
}

/// One line on a kitchen ticket.
#[derive(Debug, Clone)]
pub struct KitchenTicketLine {
    pub name: String,
    pub quantity: Quantity,
    /// The options. On a kitchen ticket these matter more than the dish name — "no nuts" is the
    /// part that hurts somebody if it is dropped — so they print indented under it, never inline.
    pub modifiers: Vec<String>,
}

#[cfg(test)]
mod kitchen_tests {
    use super::*;

    fn readable(bytes: &[u8]) -> String {
        // Consume whole ESC/POS sequences rather than blanking control bytes: `ESC a 1` would
        // otherwise leave a stray "a 1" that reads as text nobody printed.
        let mut out = String::new();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            let skip = match (byte, bytes.get(index + 1)) {
                (0x1B, Some(b'@')) => 2,
                (0x1B, Some(b'a' | b'E' | b'd' | b't')) => 3,
                (0x1B, Some(b'p')) => 5,
                (0x1D, Some(b'!')) => 3,
                (0x1D, Some(b'V')) => 4,
                _ => 0,
            };
            if skip > 0 {
                index += skip;
                continue;
            }
            if byte == b'\n' || !byte.is_ascii_control() {
                out.push(byte as char);
            }
            index += 1;
        }
        out
    }

    fn ticket(is_cancellation: bool) -> KitchenTicketData {
        KitchenTicketData {
            station: "KITCHEN".to_owned(),
            is_cancellation,
            table_label: Some("12".to_owned()),
            covers: Some(4),
            round: 2,
            printed_at: "30 Jul 2026, 19:42".to_owned(),
            lines: vec![
                KitchenTicketLine {
                    name: "Chicken curry".to_owned(),
                    quantity: Quantity::from_milli(2_000),
                    modifiers: vec!["No nuts".to_owned(), "Extra hot".to_owned()],
                },
                KitchenTicketLine {
                    name: "Naan".to_owned(),
                    quantity: Quantity::ONE,
                    modifiers: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn a_kitchen_ticket_carries_no_prices() {
        // An amount beside an item is a number somebody eventually reads as a quantity.
        let text = readable(Document::render_kitchen(&ticket(false), PaperWidth::Mm80).bytes());
        assert!(!text.contains('.'), "no decimal amounts: {text}");
        assert!(!text.to_lowercase().contains("total"));
    }

    #[test]
    fn the_table_and_round_are_on_it() {
        let text = readable(Document::render_kitchen(&ticket(false), PaperWidth::Mm80).bytes());
        assert!(text.contains("TABLE 12"));
        assert!(text.contains("Round 2"), "a cook knows the first is out");
        assert!(text.contains("4 covers"));
    }

    #[test]
    fn options_print_under_their_line_not_beside_it() {
        // Appended to a dish name they wrap into the next item on a narrow roll and read as part
        // of it — which is how "no nuts" ends up against the wrong dish.
        let text = readable(Document::render_kitchen(&ticket(false), PaperWidth::Mm58).bytes());
        assert!(text.contains("    - No nuts"));
        assert!(text.contains("    - Extra hot"));
    }

    #[test]
    fn a_cancellation_says_so_before_anything_else() {
        // The meaning of everything after it inverts, and a cook reading it late has already
        // started cooking.
        let text = readable(Document::render_kitchen(&ticket(true), PaperWidth::Mm80).bytes());
        let cancel = text.find("CANCEL").expect("marked");
        let station = text.find("KITCHEN").expect("station");
        assert!(cancel < station, "the marker comes first:\n{text}");
    }

    #[test]
    fn an_order_ticket_is_not_marked_as_a_cancellation() {
        let text = readable(Document::render_kitchen(&ticket(false), PaperWidth::Mm80).bytes());
        assert!(!text.contains("CANCEL"));
    }

    #[test]
    fn a_whole_quantity_prints_whole() {
        // "2.000 x Naan" is a quantity a cook has to parse. "2 x Naan" is one they read.
        let text = readable(Document::render_kitchen(&ticket(false), PaperWidth::Mm80).bytes());
        assert!(text.contains("2 x Chicken curry"), "{text}");
        assert!(text.contains("1 x Naan"));
    }

    #[test]
    fn every_line_fits_the_narrow_roll() {
        let text = readable(Document::render_kitchen(&ticket(false), PaperWidth::Mm58).bytes());
        for line in text.lines() {
            assert!(line.chars().count() <= 32, "too wide: {line:?}");
        }
    }

    /// Print one for a human to read. Layout is the one thing only eyes check.
    #[test]
    #[ignore = "prints a ticket to look at; run with --ignored"]
    fn dump_for_eyeballing() {
        for paper in [PaperWidth::Mm58, PaperWidth::Mm80] {
            println!("\n===== {paper:?} order =====");
            println!(
                "{}",
                readable(Document::render_kitchen(&ticket(false), paper).bytes())
            );
        }
        println!("\n===== cancellation =====");
        println!(
            "{}",
            readable(Document::render_kitchen(&ticket(true), PaperWidth::Mm58).bytes())
        );
    }

    #[test]
    fn nothing_printable_leaves_the_ascii_range() {
        // The code page is set to CP437, so a multi-byte character prints as whatever its bytes
        // happen to mean there. This caught a middle dot in the covers line that rendered as "?".
        // It does not cover Bangla or Arabic, which cannot be sent as characters at all and need
        // the raster path — see the crate note.
        for cancellation in [false, true] {
            for paper in [PaperWidth::Mm58, PaperWidth::Mm80] {
                let bytes = Document::render_kitchen(&ticket(cancellation), paper).into_bytes();
                assert!(
                    bytes.iter().all(u8::is_ascii),
                    "a non-ASCII byte reached the printer"
                );
            }
        }
    }

    #[test]
    fn the_job_is_cut_and_has_no_drawer_pulse() {
        // A prep station has no till, and a drawer that opens in a kitchen is a drawer nobody is
        // watching.
        let bytes = Document::render_kitchen(&ticket(false), PaperWidth::Mm80).into_bytes();
        assert!(bytes.windows(2).any(|pair| pair == [0x1D, b'V']), "cut");
        assert!(
            !bytes.windows(2).any(|pair| pair == [0x1B, b'p']),
            "no pulse"
        );
    }
}
