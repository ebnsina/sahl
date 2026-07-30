//! Saudi Arabia: the ZATCA simplified tax invoice, Phase 1.
//!
//! A POS receipt is a **simplified (B2C) invoice**. Phase 1 — "Generation" — asks for five facts on
//! the face of it and the same five inside a QR code: seller name, VAT registration number, the
//! timestamp, the total including VAT, and the VAT. Nothing is transmitted anywhere and nothing is
//! signed, which is why Phase 1 is compatible with a till that has no network.
//!
//! Phase 2 adds four more QR tags — the invoice hash, a cryptographic stamp, the device's public
//! key, and ZATCA's signature over it — plus UBL 2.1 XML and reporting within 24 hours. The
//! [`Tag`] numbering here leaves room for them deliberately: Phase 2 appends tags 6–9 to the same
//! TLV sequence, so nothing built now has to be unpicked.
//!
//! ## Not done here
//!
//! Phase 1 also requires the invoice to be **in Arabic**. This crate produces the facts; putting
//! Arabic on thermal paper needs joining-form shaping and RTL reordering before CP864 or a raster
//! bitmap, which is its own piece of work and not started. Until it is, a Saudi outlet is not
//! compliant on the printed side even though every figure and the QR are correct.
//!
//! ## Why this one document carries formatted strings
//!
//! Everywhere else in this crate an amount is an integer of minor units and the printer decides how
//! it looks. The QR payload is the exception, and it has to be: it is a byte sequence whose exact
//! bytes are the compliance artefact. A scanner reads "115.00", not a `Money`. So the rendering
//! happens here, once, rather than in a printer that could disagree with a screen.

use base64::Engine as _;
use sahl_core::money::{Currency, Money};
use sahl_core::tax::TaxClass;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::macros::format_description;

use crate::{FiscalError, Fiscalization, Invoice};

/// A Saudi VAT registration number is 15 digits that begin and end with 3.
const VAT_NUMBER_DIGITS: usize = 15;

/// TLV tag numbers, in the order they are concatenated.
///
/// Phase 2 continues this sequence with 6 (invoice hash), 7 (signature), 8 (public key) and
/// 9 (ZATCA's signature over the certificate's public key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Tag {
    SellerName = 1,
    VatRegistration = 2,
    Timestamp = 3,
    TotalWithVat = 4,
    VatTotal = 5,
}

/// One line of a simplified invoice.
///
/// Amounts stay integers. Only the QR is rendered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZatcaLine {
    pub description: String,
    pub unit: String,
    pub quantity_milli: i64,
    /// Excluding VAT, which is how ZATCA states a line.
    pub unit_price: Money,
    pub line_total: Money,
    pub vat_rate_basis_points: i32,
    pub vat_amount: Money,
    pub total_with_vat: Money,
}

/// A ZATCA simplified tax invoice — فاتورة ضريبية مبسطة.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimplifiedTaxInvoice {
    pub seller_name: String,
    /// The 15-digit VAT registration number.
    pub seller_vat: String,
    pub issuing_address: String,
    /// The per-device fiscal counter, rendered. ZATCA calls this the invoice reference number.
    pub invoice_number: String,
    /// Date and time both derive from this, so a reprint in another timezone cannot disagree with
    /// itself — and the QR is stamped from the same instant.
    pub issued_at_millis: i64,

    pub lines: Vec<ZatcaLine>,

    pub total_excluding_vat: Money,
    pub total_vat: Money,
    pub total_with_vat: Money,

    /// The QR payload: base64 of the concatenated TLV triplets.
    ///
    /// Stored on the document rather than recomputed at print time. The QR is part of what was
    /// issued; a second computation is a second chance to disagree with the paper already handed
    /// over.
    pub qr: String,
}

/// The ZATCA regime.
#[derive(Debug, Clone, Copy, Default)]
pub struct Zatca;

impl Fiscalization for Zatca {
    fn regime(&self) -> &'static str {
        "zatca"
    }

    fn issue(&self, invoice: &Invoice) -> Result<crate::Document, FiscalError> {
        Ok(crate::Document::Zatca(Box::new(build(invoice)?)))
    }
}

/// Build a simplified tax invoice from a completed sale.
///
/// # Errors
/// [`FiscalError`] if the sale has no lines, the seller's registration is not a Saudi VAT number,
/// or the sale is not in riyals.
pub fn build(invoice: &Invoice) -> Result<SimplifiedTaxInvoice, FiscalError> {
    if invoice.lines.is_empty() || invoice.totals.lines.is_empty() {
        return Err(FiscalError::Empty);
    }
    if invoice.seller.name.trim().is_empty() {
        return Err(FiscalError::Missing {
            field: "seller name",
            document: "ZATCA simplified tax invoice",
        });
    }
    check_vat_number(&invoice.seller.registration)?;

    // VAT is declared to ZATCA in riyals. A till configured in another currency would produce a
    // document stating a number the authority reads as SAR, which is worse than refusing to trade.
    if invoice.totals.total.currency() != Currency::Sar {
        return Err(FiscalError::Invalid(format!(
            "a ZATCA invoice is in SAR, this sale is in {}",
            invoice.totals.total.currency().code()
        )));
    }

    let mut lines = Vec::with_capacity(invoice.lines.len());
    for (line, computed) in invoice.lines.iter().zip(invoice.totals.lines.iter()) {
        lines.push(ZatcaLine {
            description: line.description.clone(),
            unit: line.unit.clone(),
            quantity_milli: line.quantity_milli,
            unit_price: unit_price(computed.net, line.quantity_milli)?,
            line_total: computed.net,
            vat_rate_basis_points: match computed.tax_class {
                TaxClass::Standard { rate } => rate.basis_points(),
                TaxClass::ZeroRated | TaxClass::Exempt => 0,
            },
            vat_amount: computed.tax,
            total_with_vat: computed.total,
        });
    }

    let qr = qr_payload(
        &invoice.seller.name,
        &invoice.seller.registration,
        invoice.issued_at.millis(),
        invoice.totals.total,
        invoice.totals.tax,
    )?;

    Ok(SimplifiedTaxInvoice {
        seller_name: invoice.seller.name.clone(),
        seller_vat: invoice.seller.registration.clone(),
        issuing_address: invoice.seller.address.clone(),
        invoice_number: invoice.sequence.to_string(),
        issued_at_millis: invoice.issued_at.millis(),
        lines,
        total_excluding_vat: invoice.totals.net,
        total_vat: invoice.totals.tax,
        total_with_vat: invoice.totals.total,
        qr,
    })
}

/// The five Phase 1 triplets, concatenated and base64-encoded.
///
/// # Errors
/// [`FiscalError::Invalid`] if a value is longer than a single length byte can describe, or the
/// timestamp is not a representable instant.
pub fn qr_payload(
    seller_name: &str,
    vat_registration: &str,
    issued_at_millis: i64,
    total_with_vat: Money,
    vat_total: Money,
) -> Result<String, FiscalError> {
    let mut bytes = Vec::new();
    push(&mut bytes, Tag::SellerName, seller_name.as_bytes())?;
    push(
        &mut bytes,
        Tag::VatRegistration,
        vat_registration.as_bytes(),
    )?;
    push(
        &mut bytes,
        Tag::Timestamp,
        zulu(issued_at_millis)?.as_bytes(),
    )?;
    push(
        &mut bytes,
        Tag::TotalWithVat,
        decimal(total_with_vat).as_bytes(),
    )?;
    push(&mut bytes, Tag::VatTotal, decimal(vat_total).as_bytes())?;

    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

/// Append one tag-length-value triplet.
///
/// The length is the value's length **in bytes, not characters**. An Arabic seller name is two
/// bytes per letter in UTF-8, so counting characters would understate every length and every
/// scanner would read the payload as corrupt from that tag onward.
fn push(buffer: &mut Vec<u8>, tag: Tag, value: &[u8]) -> Result<(), FiscalError> {
    let length = u8::try_from(value.len()).map_err(|_| {
        FiscalError::Invalid(format!(
            "a QR field is at most 255 bytes, tag {} is {}",
            tag as u8,
            value.len()
        ))
    })?;

    buffer.push(tag as u8);
    buffer.push(length);
    buffer.extend_from_slice(value);
    Ok(())
}

/// ISO 8601 in UTC with a `Z`, which is what ZATCA's own examples show.
///
/// Not RFC 3339's `+00:00` spelling: the two mean the same instant and are not the same bytes, and
/// the QR is compared byte for byte.
fn zulu(millis: i64) -> Result<String, FiscalError> {
    let seconds = millis.div_euclid(1_000);
    let moment = OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|_| FiscalError::Invalid(format!("{millis} is not a representable instant")))?;

    moment
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
        ))
        .map_err(|error| FiscalError::Invalid(error.to_string()))
}

/// A plain decimal with the currency's own number of places — `115.00`, never `115` or `١١٥`.
///
/// ZATCA requires Arabic *numerals* in the Western sense (1234567890), so this must never route
/// through a locale-aware formatter.
fn decimal(amount: Money) -> String {
    let places = usize::from(amount.currency().exponent());
    let per_major = amount.currency().minor_per_major();
    let minor = amount.minor();
    let sign = if minor < 0 { "-" } else { "" };
    let magnitude = minor.unsigned_abs();
    let major = magnitude.div_euclid(per_major.unsigned_abs());
    let rest = magnitude.rem_euclid(per_major.unsigned_abs());

    if places == 0 {
        format!("{sign}{major}")
    } else {
        format!("{sign}{major}.{rest:0places$}")
    }
}

/// A Saudi VAT registration number: 15 digits, beginning and ending with 3.
fn check_vat_number(registration: &str) -> Result<(), FiscalError> {
    let trimmed = registration.trim();
    if trimmed.is_empty() {
        return Err(FiscalError::Missing {
            field: "VAT registration number",
            document: "ZATCA simplified tax invoice",
        });
    }
    if trimmed.len() != VAT_NUMBER_DIGITS
        || !trimmed.chars().all(|c| c.is_ascii_digit())
        || !trimmed.starts_with('3')
        || !trimmed.ends_with('3')
    {
        // Checked at issue rather than at setup as well, because a registration copied wrong is
        // wrong on every invoice at once and the QR is what an inspector scans.
        return Err(FiscalError::Invalid(format!(
            "{trimmed} is not a Saudi VAT number: 15 digits beginning and ending with 3"
        )));
    }
    Ok(())
}

/// The excluding-VAT unit price, spread back over the quantity.
///
/// Derived, and the line total is the exact figure — on a weighed line, quantity × unit price
/// cannot come back to the net, so the rounding is put where it does no damage.
fn unit_price(net: Money, quantity_milli: i64) -> Result<Money, FiscalError> {
    if quantity_milli == 0 {
        return Ok(Money::from_minor(0, net.currency()));
    }
    Ok(net.mul_ratio(
        sahl_core::quantity::Quantity::MILLI_PER_UNIT,
        quantity_milli,
        sahl_core::money::Rounding::HalfUp,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::invoice;
    use crate::{Buyer, FiscalLine, Seller};
    use sahl_core::quantity::Quantity;
    use sahl_core::tax::{Discount, LineInput, OrderInput, calculate};
    use uuid::Uuid;

    const SAR: Currency = Currency::Sar;

    fn seller() -> Seller {
        Seller {
            name: "Al Faisaliah Market".to_owned(),
            registration: "300000000000003".to_owned(),
            address: "King Fahd Road, Riyadh 12211".to_owned(),
        }
    }

    /// One line, 100.00 SAR before VAT at 15%.
    fn saudi_invoice() -> Invoice {
        let totals = calculate(&OrderInput::new(
            SAR,
            vec![LineInput {
                unit_price: Money::from_minor(11_500, SAR),
                quantity: Quantity::ONE,
                tax_class: TaxClass::standard(1500),
                discount: Discount::None,
            }],
        ))
        .expect("calculates");

        Invoice {
            sale_id: Uuid::from_u128(1),
            sequence: 42,
            issued_at: sahl_core::time::Timestamp::from_millis(1_650_900_600_000),
            seller: seller(),
            buyer: Buyer::default(),
            lines: vec![FiscalLine {
                description: "Dates, 1kg".to_owned(),
                unit: "pcs".to_owned(),
                quantity_milli: 1_000,
            }],
            totals,
            destination: None,
        }
    }

    fn decode(qr: &str) -> Vec<(u8, Vec<u8>)> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(qr)
            .expect("base64");
        let mut triplets = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            let tag = bytes[index];
            let length = usize::from(bytes[index + 1]);
            triplets.push((tag, bytes[index + 2..index + 2 + length].to_vec()));
            index += 2 + length;
        }
        triplets
    }

    fn field(qr: &str, tag: Tag) -> String {
        let triplets = decode(qr);
        let found = triplets
            .iter()
            .find(|(number, _)| *number == tag as u8)
            .expect("tag present");
        String::from_utf8(found.1.clone()).expect("utf-8")
    }

    #[test]
    fn the_qr_carries_the_five_phase_one_tags_in_order() {
        let document = build(&saudi_invoice()).expect("builds");
        let tags: Vec<u8> = decode(&document.qr)
            .into_iter()
            .map(|(tag, _)| tag)
            .collect();

        assert_eq!(tags, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn the_qr_states_the_seller_and_the_registration() {
        let document = build(&saudi_invoice()).expect("builds");

        assert_eq!(field(&document.qr, Tag::SellerName), "Al Faisaliah Market");
        assert_eq!(field(&document.qr, Tag::VatRegistration), "300000000000003");
    }

    #[test]
    fn the_timestamp_is_zulu_rather_than_rfc_3339s_offset() {
        // The two mean the same instant and are not the same bytes, and the QR is compared byte
        // for byte.
        let document = build(&saudi_invoice()).expect("builds");
        assert_eq!(field(&document.qr, Tag::Timestamp), "2022-04-25T15:30:00Z");
    }

    #[test]
    fn the_totals_are_plain_two_place_decimals() {
        let document = build(&saudi_invoice()).expect("builds");

        assert_eq!(field(&document.qr, Tag::TotalWithVat), "115.00");
        assert_eq!(field(&document.qr, Tag::VatTotal), "15.00");
    }

    #[test]
    fn an_arabic_seller_name_is_measured_in_bytes_not_characters() {
        // Two bytes per Arabic letter in UTF-8. Counting characters would understate the length
        // and every scanner would read the payload as corrupt from that tag onward.
        let mut invoice = saudi_invoice();
        invoice.seller.name = "متجر الفيصلية".to_owned();
        let document = build(&invoice).expect("builds");

        assert_eq!(field(&document.qr, Tag::SellerName), "متجر الفيصلية");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&document.qr)
            .expect("base64");
        assert_eq!(
            usize::from(bytes[1]),
            "متجر الفيصلية".len(),
            "the length byte counts bytes"
        );
    }

    #[test]
    fn a_name_too_long_for_one_length_byte_is_refused_rather_than_truncated() {
        // A truncated QR still scans. It just says something other than what was sold.
        let mut invoice = saudi_invoice();
        invoice.seller.name = "ن".repeat(200);

        assert!(matches!(build(&invoice), Err(FiscalError::Invalid(_))));
    }

    #[test]
    fn a_registration_that_is_not_a_saudi_vat_number_is_refused() {
        for wrong in [
            "0031234567890",   // a Bangladeshi BIN
            "30000000000000",  // fourteen digits
            "400000000000003", // does not begin with 3
            "300000000000004", // does not end with 3
            "30000000000000X", // not digits
        ] {
            let mut invoice = saudi_invoice();
            invoice.seller.registration = wrong.to_owned();
            assert!(build(&invoice).is_err(), "{wrong} was accepted");
        }
    }

    #[test]
    fn a_sale_in_another_currency_is_refused() {
        // The document would state a number ZATCA reads as riyals.
        assert!(matches!(build(&invoice(1)), Err(FiscalError::Invalid(_))));
    }

    #[test]
    fn an_empty_sale_issues_nothing() {
        let mut invoice = saudi_invoice();
        invoice.lines.clear();
        assert_eq!(build(&invoice), Err(FiscalError::Empty));
    }

    #[test]
    fn the_line_is_stated_excluding_vat() {
        // A tax-inclusive shelf price of 115.00 is 100.00 in the line and 15.00 in the VAT column.
        let document = build(&saudi_invoice()).expect("builds");
        let line = &document.lines[0];

        assert_eq!(line.line_total, Money::from_minor(10_000, SAR));
        assert_eq!(line.unit_price, Money::from_minor(10_000, SAR));
        assert_eq!(line.vat_amount, Money::from_minor(1_500, SAR));
        assert_eq!(line.total_with_vat, Money::from_minor(11_500, SAR));
        assert_eq!(line.vat_rate_basis_points, 1500);
    }

    #[test]
    fn the_document_totals_match_the_tax_engine() {
        let document = build(&saudi_invoice()).expect("builds");

        assert_eq!(document.total_excluding_vat, Money::from_minor(10_000, SAR));
        assert_eq!(document.total_vat, Money::from_minor(1_500, SAR));
        assert_eq!(document.total_with_vat, Money::from_minor(11_500, SAR));
    }

    #[test]
    fn a_zero_rated_line_states_a_zero_rate_and_no_vat() {
        let totals = calculate(&OrderInput::new(
            SAR,
            vec![LineInput {
                unit_price: Money::from_minor(5_000, SAR),
                quantity: Quantity::ONE,
                tax_class: TaxClass::ZeroRated,
                discount: Discount::None,
            }],
        ))
        .expect("calculates");
        let invoice = Invoice {
            totals,
            ..saudi_invoice()
        };

        let document = build(&invoice).expect("builds");
        assert_eq!(document.lines[0].vat_rate_basis_points, 0);
        assert_eq!(document.total_vat, Money::from_minor(0, SAR));
    }

    #[test]
    fn the_regime_names_itself() {
        assert_eq!(Zatca.regime(), "zatca");
    }

    #[test]
    fn issuing_through_the_trait_produces_the_same_document() {
        let invoice = saudi_invoice();
        let crate::Document::Zatca(issued) = Zatca.issue(&invoice).expect("issues") else {
            panic!("wrong document");
        };
        assert_eq!(*issued, build(&invoice).expect("builds"));
    }

    #[test]
    fn a_decimal_never_loses_its_places() {
        assert_eq!(decimal(Money::from_minor(0, SAR)), "0.00");
        assert_eq!(decimal(Money::from_minor(5, SAR)), "0.05");
        assert_eq!(decimal(Money::from_minor(100, SAR)), "1.00");
        assert_eq!(decimal(Money::from_minor(-1_234, SAR)), "-12.34");
    }
}
