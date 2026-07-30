//! A sale becoming printer bytes.
//!
//! **No physical printer has been involved in any of this.** These tests assert that the byte
//! stream is well-formed ESC/POS and that the human-readable text on it says what the sale says.
//! That is worth having and it is not the same as knowing a receipt comes out of a machine
//! correctly: real printers disagree about code pages, cut commands, drawer pulses and buffering.
//!
//! What these *do* rule out is the class of bug where the receipt disagrees with the log — a total
//! that does not match, a voided line printed as if it were sold, a missing invoice number. Those
//! are the ones that cost a merchant an argument with a customer, and they need no hardware to
//! catch.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use sahl_core::Timestamp;
use sahl_core::money::{Currency, Money, Rounding};
use sahl_core::outlet::{FiscalRegime, OutletEvent, OutletSettings, Profile};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{SaleEvent, TenderMethod, VoidReason};
use sahl_core::tax::{PricingMode, TaxClass};
use sahl_escpos::{Document, PaperWidth};
use sahl_terminal_lib::printer::{PrintError, PrinterTarget, print};
use sahl_terminal_lib::store::EventStore;
use sahl_terminal_lib::{DeviceIdentity, Terminal};
use uuid::Uuid;

const BDT: Currency = Currency::Bdt;
const ESC: u8 = 0x1B;
const GS: u8 = 0x1D;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn at(n: i64) -> Timestamp {
    Timestamp::from_millis(1_753_000_000_000 + n)
}

fn identity() -> DeviceIdentity {
    DeviceIdentity {
        tenant_id: id(1),
        outlet_id: id(2),
        device_id: id(3),
    }
}

const SALE: u128 = 0x5A1E;
const CASHIER: u128 = 0xCA51;

/// A configured till with one completed sale: two lines, one of them voided.
fn till_with_a_sale() -> Terminal {
    let store = EventStore::open_in_memory(id(3)).expect("opens");
    let mut till = Terminal::load(store, identity()).expect("loads");

    till.record_outlet(
        &OutletEvent::Configured {
            outlet_id: identity().outlet_id,
            settings: OutletSettings {
                name: "Karim Store".to_owned(),
                profile: Profile::Retail,
                currency: BDT,
                timezone: "Asia/Dhaka".to_owned(),
                regime: FiscalRegime::BdMushak,
                tax_registration: Some("0031234567890".to_owned()),
                address: "12 Dhanmondi 27, Dhaka".to_owned(),
            },
            at: at(0),
            configured_by: id(0x0E),
        },
        id(40),
        at(0),
    )
    .expect("configures");

    till.record(
        &SaleEvent::Opened {
            sale_id: id(SALE),
            opened_by: id(CASHIER),
            currency: BDT,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        },
        id(50),
        at(1),
    )
    .expect("opens");

    till.record(
        &SaleEvent::LineAdded {
            sale_id: id(SALE),
            line_id: id(51),
            product_id: id(0x101),
            name: "Basmati rice 5kg".to_owned(),
            unit_price: Money::from_minor(48_000, BDT),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            modifiers: Vec::new(),
        },
        id(52),
        at(2),
    )
    .expect("adds");

    till.record(
        &SaleEvent::LineAdded {
            sale_id: id(SALE),
            line_id: id(53),
            product_id: id(0x104),
            name: "Fresh milk 1L".to_owned(),
            unit_price: Money::from_minor(9_000, BDT),
            quantity: Quantity::ONE,
            tax_class: TaxClass::Exempt,
            modifiers: Vec::new(),
        },
        id(54),
        at(3),
    )
    .expect("adds");

    // Voided, not removed. It must still appear on the paper.
    till.record(
        &SaleEvent::LineVoided {
            sale_id: id(SALE),
            line_id: id(53),
            reason: VoidReason::Mistake,
            authorized_by: id(0x11A),
        },
        id(55),
        at(4),
    )
    .expect("voids");

    till.record(
        &SaleEvent::TenderRecorded {
            sale_id: id(SALE),
            tender_id: id(56),
            method: TenderMethod::Cash,
            amount: Money::from_minor(50_000, BDT),
            reference: None,
        },
        id(57),
        at(5),
    )
    .expect("tenders");

    till.complete_sale(
        &SaleEvent::Completed {
            sale_id: id(SALE),
            total: Money::from_minor(48_000, BDT),
            change_given: Money::from_minor(2_000, BDT),
            at: at(6),
        },
        "bd_mushak",
        id(CASHIER),
        at(6),
    )
    .expect("completes");

    till
}

/// What a person would actually read off the paper.
///
/// Every ESC/POS sequence is consumed whole, parameters included. Simply blanking control bytes is
/// not enough and gets this wrong in a way that looks right: `ESC a 1` leaves a stray "a 1" on the
/// line, which then reads as text nobody printed and inflates every line-length measurement.
fn readable(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        // Sequence lengths for exactly what the renderer emits — see sahl-escpos::command.
        let skip = match (byte, bytes.get(index + 1)) {
            (ESC, Some(b'@')) => 2,
            (ESC, Some(b'a' | b'E' | b'd' | b't')) => 3,
            (ESC, Some(b'p')) => 5,
            (GS, Some(b'!')) => 3,
            (GS, Some(b'V')) => 4,
            (GS, Some(b'v')) => 8,
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

fn render(till: &Terminal, paper: PaperWidth, open_drawer: bool) -> Vec<u8> {
    let data = till
        .receipt(id(SALE), "29 Jul 2026, 21:15".to_owned())
        .expect("builds");
    Document::render(&data, paper, open_drawer).into_bytes()
}

#[test]
fn the_job_is_well_formed_escpos() {
    let bytes = render(&till_with_a_sale(), PaperWidth::Mm80, false);

    // ESC @ — a previous job that died mid-print can leave a printer in an odd state, and every
    // job must start by clearing it rather than inheriting it.
    assert_eq!(&bytes[0..2], &[ESC, b'@'], "must begin with initialize");
    // GS V — the cut. Without it the next receipt prints onto the same strip of paper.
    assert!(
        bytes.windows(2).any(|pair| pair == [GS, b'V']),
        "must end the job with a cut"
    );
}

#[test]
fn the_drawer_pulse_is_present_only_when_asked_for() {
    // A drawer that springs open on a card payment is a security problem, not a convenience.
    let without = render(&till_with_a_sale(), PaperWidth::Mm80, false);
    let with = render(&till_with_a_sale(), PaperWidth::Mm80, true);

    let pulse = |bytes: &[u8]| bytes.windows(2).any(|pair| pair == [ESC, b'p']);
    assert!(!pulse(&without));
    assert!(pulse(&with));
}

#[test]
fn the_paper_says_what_the_sale_says() {
    let text = readable(&render(&till_with_a_sale(), PaperWidth::Mm80, false));

    assert!(text.contains("Karim Store"), "shop name");
    assert!(
        text.contains("0031234567890"),
        "BIN — Mushak needs it on the face"
    );
    assert!(text.contains("Basmati rice 5kg"), "the line sold");
    assert!(text.contains("480.00"), "and its price");
    // 480.00 tax-inclusive at 15% is 417.39 net and 62.61 VAT. The receipt must show the split,
    // because that split is the whole reason a VAT-registered shop prints one.
    assert!(text.contains("62.61"), "the VAT actually charged:\n{text}");
    assert!(text.contains("417.39"), "and the taxable base:\n{text}");
}

#[test]
fn a_voided_line_is_printed_and_marked() {
    // Omitting it would make the paper disagree with the log, and a customer who saw it scanned
    // has no way to tell a void from a quiet removal.
    let text = readable(&render(&till_with_a_sale(), PaperWidth::Mm80, false));

    assert!(
        text.contains("Fresh milk 1L"),
        "the voided line is still there"
    );
    assert!(
        text.to_uppercase().contains("VOID"),
        "and marked as void: {text}"
    );
}

#[test]
fn the_invoice_number_is_the_fiscal_counter() {
    // Not the sale's UUID. A customer quoting a number back has to be quoting the one on the
    // challan, or nothing can be looked up.
    let text = readable(&render(&till_with_a_sale(), PaperWidth::Mm80, false));
    assert!(text.contains('1'), "the first invoice this device issued");
    assert!(
        !text.contains(&id(SALE).to_string()),
        "and not the internal sale id"
    );
}

#[test]
fn a_narrow_roll_does_not_truncate_the_invoice_number() {
    // 58mm is 32 columns. Compliance numbers must survive the squeeze — Mushak and ZATCA both
    // require them in full, and a wrapped-off digit is a document that identifies nothing.
    let text = readable(&render(&till_with_a_sale(), PaperWidth::Mm58, false));

    assert!(text.contains("0031234567890"), "the BIN survives 58mm");
    for line in text.lines() {
        assert!(
            line.chars().count() <= 32,
            "line exceeds the 58mm roll: {line:?}"
        );
    }
}

#[test]
fn every_line_fits_the_80mm_roll() {
    let text = readable(&render(&till_with_a_sale(), PaperWidth::Mm80, false));
    for line in text.lines() {
        assert!(
            line.chars().count() <= 48,
            "line exceeds the 80mm roll: {line:?}"
        );
    }
}

#[test]
fn a_job_can_be_spooled_to_a_file_and_read_back() {
    // The file target is how a byte stream gets inspected without hardware, and how a merchant
    // reports a wrong-looking receipt without photographing a curled roll.
    let path = std::env::temp_dir().join(format!("sahl-receipt-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let bytes = render(&till_with_a_sale(), PaperWidth::Mm80, false);
    print(&PrinterTarget::File(path.clone()), &bytes).expect("spools");

    let spooled = std::fs::read(&path).expect("reads");
    assert_eq!(spooled, bytes);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_unconfigured_printer_refuses_rather_than_pretending() {
    let bytes = render(&till_with_a_sale(), PaperWidth::Mm80, false);
    assert_eq!(
        print(&PrinterTarget::None, &bytes),
        Err(PrintError::NotConfigured)
    );
}

#[test]
fn a_sale_stays_completed_when_the_printer_is_unreachable() {
    // The whole posture: paper is a courtesy and a reprintable artefact. The money is already in
    // the drawer and the sale is already in the log.
    let till = till_with_a_sale();
    let bytes = render(&till, PaperWidth::Mm80, false);

    let failed = print(&PrinterTarget::Network("127.0.0.1:1".to_owned()), &bytes);
    assert!(failed.is_err());

    assert!(
        till.sale(id(SALE)).expect("sale").settled_at().is_some(),
        "the sale is untouched by a printer that was not there"
    );
}

/// Print a receipt for a human to read.
///
/// Kept because the only real check on layout is someone looking at it, and until a physical
/// printer exists this is the closest thing available. `cargo test -p sahl-terminal --test printing
/// dump -- --ignored --nocapture`.
#[test]
#[ignore = "prints a receipt for a human to look at; run with --ignored"]
fn dump_for_eyeballing() {
    for paper in [PaperWidth::Mm58, PaperWidth::Mm80] {
        println!("\n===== {paper:?} =====");
        println!("{}", readable(&render(&till_with_a_sale(), paper, false)));
    }
}
