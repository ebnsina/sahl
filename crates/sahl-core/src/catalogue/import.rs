//! Turning a spreadsheet into a catalogue.
//!
//! The demo that converts: a shopkeeper arrives with the file their supplier sends, or an export
//! from whatever they use now, and is selling within the quarter of an hour. Nothing here is
//! clever — it is the unglamorous work of reading somebody else's columns without losing a row.
//!
//! ## Nothing is imported until all of it parses
//!
//! A partial import is the worst outcome available. Half a catalogue looks like a working shop, so
//! the missing half is discovered at the counter, one product at a time, by a cashier who cannot
//! fix it. So this reports every problem with its line number and imports nothing until they are
//! resolved — a spreadsheet is easy to correct and re-upload, and a half-populated till is not
//! easy to do anything with.
//!
//! ## No column is guessed
//!
//! Headers are matched by name, case-insensitively, with a few common spellings accepted. A
//! positional fallback would silently read a supplier's cost column as a shelf price the first
//! time somebody exported their columns in a different order.

use serde::{Deserialize, Serialize};

use crate::money::{Currency, Money};
use crate::tax::TaxClass;

use super::product::Unit;

/// A row that parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedProduct {
    pub name: String,
    pub sku: Option<String>,
    pub barcodes: Vec<String>,
    pub price: Money,
    pub unit: Unit,
    pub tax_class: TaxClass,
    pub category: Option<String>,
}

/// Something wrong with one row, named well enough to fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportProblem {
    /// One-based and counting the header, so it matches what the spreadsheet shows.
    pub line: usize,
    pub column: &'static str,
    pub found: String,
    pub because: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("the file is empty")]
    Empty,

    #[error("no header row — the first line must name the columns")]
    NoHeader,

    #[error("a {column} column is required")]
    MissingColumn { column: &'static str },

    #[error("{count} rows could not be read")]
    Rows { count: usize },
}

/// What a file turned out to contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub products: Vec<ImportedProduct>,
    /// Empty when everything parsed. Nothing is imported while this is not.
    pub problems: Vec<ImportProblem>,
}

impl Import {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Which column holds what, resolved from the header row.
#[derive(Debug, Default)]
struct Columns {
    name: Option<usize>,
    price: Option<usize>,
    sku: Option<usize>,
    barcode: Option<usize>,
    unit: Option<usize>,
    vat: Option<usize>,
    category: Option<usize>,
}

impl Columns {
    /// Accepts the spellings a real export uses. Anything unrecognised is ignored rather than
    /// refused: a supplier's file carries columns this program has no use for, and rejecting the
    /// file over a `Reorder Level` column would be absurd.
    fn read(header: &[String]) -> Self {
        let mut columns = Self::default();
        for (index, raw) in header.iter().enumerate() {
            let name = raw.trim().to_lowercase();
            let slot = match name.as_str() {
                "name" | "product" | "product name" | "description" | "item" => &mut columns.name,
                "price" | "unit price" | "selling price" | "mrp" | "rate" => &mut columns.price,
                "sku" | "code" | "item code" | "product code" => &mut columns.sku,
                "barcode" | "barcodes" | "ean" | "upc" => &mut columns.barcode,
                "unit" | "uom" | "unit of supply" => &mut columns.unit,
                "vat" | "vat %" | "vat rate" | "tax" | "tax rate" => &mut columns.vat,
                "category" | "group" | "department" => &mut columns.category,
                _ => continue,
            };
            // First wins. A file with two `Price` columns is ambiguous, and quietly taking the
            // last one would depend on column order nobody chose deliberately.
            if slot.is_none() {
                *slot = Some(index);
            }
        }
        columns
    }
}

/// Read a delimited file into products.
///
/// `currency` comes from the outlet, never from the file — a column of bare numbers says nothing
/// about what they are denominated in, and guessing would put riyal prices in a taka shop.
///
/// # Errors
/// [`ImportError`] when the file has no header or is missing a column nothing can be inferred
/// from. Row-level problems come back in [`Import::problems`] rather than as an error, because the
/// point is to show all of them at once.
pub fn from_delimited(
    text: &str,
    delimiter: char,
    currency: Currency,
    default_vat_basis_points: i32,
) -> Result<Import, ImportError> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());

    let header_line = lines.next().ok_or(ImportError::Empty)?;
    let header: Vec<String> = split(header_line, delimiter);
    let columns = Columns::read(&header);

    let Some(name_at) = columns.name else {
        return Err(ImportError::MissingColumn { column: "name" });
    };
    let Some(price_at) = columns.price else {
        return Err(ImportError::MissingColumn { column: "price" });
    };

    let mut products = Vec::new();
    let mut problems = Vec::new();

    for (offset, raw) in lines.enumerate() {
        // Plus two: one for the header, one because spreadsheets count from one.
        let line = offset.saturating_add(2);
        let cells = split(raw, delimiter);
        let cell = |at: Option<usize>| -> Option<String> {
            at.and_then(|index| cells.get(index))
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };

        let Some(name) = cell(Some(name_at)) else {
            problems.push(ImportProblem {
                line,
                column: "name",
                found: String::new(),
                because: "every product needs a name".to_owned(),
            });
            continue;
        };

        let raw_price = cell(Some(price_at)).unwrap_or_default();
        let price = match parse_money(&raw_price, currency) {
            Ok(value) => value,
            Err(because) => {
                problems.push(ImportProblem {
                    line,
                    column: "price",
                    found: raw_price,
                    because,
                });
                continue;
            }
        };

        let unit = match cell(columns.unit) {
            None => Unit::Piece,
            Some(raw_unit) => match parse_unit(&raw_unit) {
                Some(value) => value,
                None => {
                    problems.push(ImportProblem {
                        line,
                        column: "unit",
                        found: raw_unit,
                        because: "not a unit this till knows — try pcs, kg, g, L, ml, m or pack"
                            .to_owned(),
                    });
                    continue;
                }
            },
        };

        let tax_class = match cell(columns.vat) {
            None => TaxClass::standard(default_vat_basis_points),
            Some(raw_vat) => match parse_vat(&raw_vat) {
                Some(value) => value,
                None => {
                    problems.push(ImportProblem {
                        line,
                        column: "vat",
                        found: raw_vat,
                        because: "expected a percentage, or 'exempt' or 'zero'".to_owned(),
                    });
                    continue;
                }
            },
        };

        products.push(ImportedProduct {
            name,
            sku: cell(columns.sku),
            // Several barcodes in one cell is ordinary; a supplier separates them with a space or
            // a semicolon depending on who wrote their export.
            barcodes: cell(columns.barcode)
                .map(|value| {
                    value
                        .split([';', ' ', '|'])
                        .map(str::trim)
                        .filter(|part| !part.is_empty())
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            price,
            unit,
            tax_class,
            category: cell(columns.category),
        });
    }

    Ok(Import { products, problems })
}

/// Split a line, honouring double quotes.
///
/// A product called `Rice, loose` is not two columns, and a shop selling one would otherwise find
/// every row after it shifted by one — silently, because the row count would still be right.
fn split(line: &str, delimiter: char) -> Vec<String> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut quoted = false;

    for character in line.chars() {
        match character {
            '"' => quoted = !quoted,
            _ if character == delimiter && !quoted => {
                cells.push(current.trim().to_owned());
                current = String::new();
            }
            _ => current.push(character),
        }
    }
    cells.push(current.trim().to_owned());
    cells
}

/// Read a price written the way a person writes one.
fn parse_money(raw: &str, currency: Currency) -> Result<Money, String> {
    // Thousands separators and a currency symbol are what a spreadsheet exports; refusing them
    // would send somebody to find-and-replace before they could try the product.
    let cleaned: String = raw
        .chars()
        .filter(|character| character.is_ascii_digit() || *character == '.' || *character == '-')
        .collect();

    if cleaned.is_empty() {
        return Err("a price is required".to_owned());
    }

    let exponent = u32::from(currency.exponent());
    let (whole, fraction) = cleaned.split_once('.').unwrap_or((cleaned.as_str(), ""));

    if fraction.len() > exponent as usize {
        return Err(format!(
            "{} has more than {exponent} decimal places",
            raw.trim()
        ));
    }

    let scale = 10_i64.pow(exponent);
    let major: i64 = whole
        .parse()
        .map_err(|_| format!("{} is not a number", raw.trim()))?;
    let minor: i64 = if fraction.is_empty() {
        0
    } else {
        let padded = format!("{fraction:0<width$}", width = exponent as usize);
        padded
            .parse()
            .map_err(|_| format!("{} is not a number", raw.trim()))?
    };

    // Refused before the sign can matter. A negative price is never wanted, and handling one
    // would mean deciding whether the minus belongs to the whole part or the whole amount.
    if major < 0 || cleaned.starts_with('-') {
        return Err("a price cannot be negative".to_owned());
    }

    let total = major
        .checked_mul(scale)
        .and_then(|value| value.checked_add(minor))
        .ok_or_else(|| format!("{} is too large", raw.trim()))?;

    Ok(Money::from_minor(total, currency))
}

fn parse_unit(raw: &str) -> Option<Unit> {
    match raw.trim().to_lowercase().as_str() {
        "pcs" | "pc" | "piece" | "pieces" | "each" | "ea" | "unit" | "" => Some(Unit::Piece),
        "kg" | "kilo" | "kilogram" | "kgs" => Some(Unit::Kilogram),
        "g" | "gram" | "grams" | "gm" => Some(Unit::Gram),
        "l" | "litre" | "liter" | "ltr" => Some(Unit::Litre),
        "ml" | "millilitre" | "milliliter" => Some(Unit::Millilitre),
        "m" | "metre" | "meter" => Some(Unit::Metre),
        "pack" | "packet" | "box" | "carton" => Some(Unit::Pack),
        _ => None,
    }
}

/// Read a VAT column.
///
/// The three treatments are legally different and only one of them is a rate, so `exempt` and
/// `zero` are spelled rather than expressed as 0% — a zero-rated supply keeps input VAT
/// reclaimable and an exempt one does not.
fn parse_vat(raw: &str) -> Option<TaxClass> {
    let cleaned = raw.trim().to_lowercase();
    match cleaned.as_str() {
        "exempt" | "exempted" | "e" => return Some(TaxClass::Exempt),
        "zero" | "zero-rated" | "zero rated" | "z" => return Some(TaxClass::ZeroRated),
        _ => {}
    }

    let digits: String = cleaned
        .chars()
        .filter(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    if digits.is_empty() {
        return None;
    }

    let (whole, fraction) = digits.split_once('.').unwrap_or((digits.as_str(), ""));
    let percent: i32 = whole.parse().ok()?;
    let hundredths: i32 = match fraction.len() {
        0 => 0,
        _ => format!("{fraction:0<2}").get(..2)?.parse().ok()?,
    };

    let basis_points = percent.checked_mul(100)?.checked_add(hundredths)?;
    if !(0..=10_000).contains(&basis_points) {
        return None;
    }
    Some(TaxClass::standard(basis_points))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BDT: Currency = Currency::Bdt;

    fn read(text: &str) -> Import {
        from_delimited(text, ',', BDT, 1500).expect("parses")
    }

    #[test]
    fn an_ordinary_export_becomes_a_catalogue() {
        let import = read("Name,Price,SKU,Unit\nRice,46.00,R1,kg\nSoap,55,S1,pcs\n");

        assert!(import.is_clean());
        assert_eq!(import.products.len(), 2);
        assert_eq!(import.products[0].price, Money::from_minor(4_600, BDT));
        assert_eq!(import.products[0].unit, Unit::Kilogram);
        assert_eq!(import.products[1].price, Money::from_minor(5_500, BDT));
    }

    #[test]
    fn columns_are_matched_by_name_in_any_order() {
        // A positional fallback would read a supplier's cost column as a shelf price the first
        // time somebody exported their columns differently.
        let import = read("SKU,Unit,Price,Name\nR1,kg,46.00,Rice\n");

        assert!(import.is_clean());
        assert_eq!(import.products[0].name, "Rice");
        assert_eq!(import.products[0].sku.as_deref(), Some("R1"));
    }

    #[test]
    fn the_spellings_a_real_export_uses_are_understood() {
        let import = read("Product Name,MRP,Item Code,UOM\nRice,46.00,R1,KG\n");
        assert!(import.is_clean(), "{:?}", import.problems);
        assert_eq!(import.products[0].unit, Unit::Kilogram);
    }

    #[test]
    fn a_column_nothing_uses_is_ignored_rather_than_refused() {
        // A supplier's file carries columns this program has no use for.
        let import = read("Name,Price,Reorder Level,Supplier\nRice,46.00,20,Acme\n");
        assert!(import.is_clean());
        assert_eq!(import.products.len(), 1);
    }

    #[test]
    fn a_comma_inside_a_quoted_name_is_not_a_column_break() {
        // Otherwise every row after it shifts by one — silently, because the row count is right.
        let import = read("Name,Price\n\"Rice, loose\",46.00\n");

        assert!(import.is_clean(), "{:?}", import.problems);
        assert_eq!(import.products[0].name, "Rice, loose");
        assert_eq!(import.products[0].price, Money::from_minor(4_600, BDT));
    }

    #[test]
    fn a_price_written_the_way_a_person_writes_one_is_read() {
        let import = read("Name,Price\nRice,\"৳ 1,234.50\"\n");
        assert!(import.is_clean(), "{:?}", import.problems);
        assert_eq!(import.products[0].price, Money::from_minor(123_450, BDT));
    }

    #[test]
    fn a_price_with_too_many_decimals_is_refused_rather_than_rounded() {
        // Rounding somebody's file for them is how a catalogue ends up a paisa off everywhere and
        // nobody knows which step did it.
        let import = read("Name,Price\nRice,46.005\n");

        assert_eq!(import.problems.len(), 1);
        assert_eq!(import.problems[0].column, "price");
        assert_eq!(import.problems[0].line, 2);
    }

    #[test]
    fn every_bad_row_is_reported_not_just_the_first() {
        // A file is easy to correct and re-upload. Fixing one error per attempt is not.
        let import = read("Name,Price\nRice,nonsense\nSoap,also nonsense\nOil,46.00\n");

        assert_eq!(import.problems.len(), 2);
        assert_eq!(import.problems[0].line, 2);
        assert_eq!(import.problems[1].line, 3);
        assert!(!import.is_clean(), "nothing is imported while these stand");
    }

    #[test]
    fn a_line_number_matches_what_the_spreadsheet_shows() {
        let import = read("Name,Price\nRice,46.00\nSoap,broken\n");
        assert_eq!(import.problems[0].line, 3, "header is line one");
    }

    #[test]
    fn a_row_with_no_name_is_a_problem_rather_than_a_blank_product() {
        let import = read("Name,Price\n,46.00\n");
        assert_eq!(import.problems[0].column, "name");
    }

    #[test]
    fn exempt_and_zero_rated_are_spelled_rather_than_written_as_zero() {
        // Legally different: zero-rated keeps input VAT reclaimable and exempt does not.
        let import = read("Name,Price,VAT\nMilk,90,zero\nRent,100,exempt\nRice,46,15\n");

        assert!(import.is_clean(), "{:?}", import.problems);
        assert_eq!(import.products[0].tax_class, TaxClass::ZeroRated);
        assert_eq!(import.products[1].tax_class, TaxClass::Exempt);
        assert_eq!(import.products[2].tax_class, TaxClass::standard(1500));
    }

    #[test]
    fn a_missing_vat_column_uses_the_outlets_standard_rate() {
        let import = read("Name,Price\nRice,46.00\n");
        assert_eq!(import.products[0].tax_class, TaxClass::standard(1500));
    }

    #[test]
    fn several_barcodes_in_one_cell_all_arrive() {
        let import = read("Name,Price,Barcode\nRice,46,\"8901 8902;8903\"\n");
        assert_eq!(import.products[0].barcodes.len(), 3);
    }

    #[test]
    fn a_file_with_no_name_column_is_refused_outright() {
        // Not a row problem — nothing in the file can be read at all, and reporting it per row
        // would print the same message a thousand times.
        assert_eq!(
            from_delimited("Price,SKU\n46.00,R1\n", ',', BDT, 1500),
            Err(ImportError::MissingColumn { column: "name" })
        );
    }

    #[test]
    fn an_empty_file_says_so() {
        assert_eq!(from_delimited("", ',', BDT, 1500), Err(ImportError::Empty));
    }

    #[test]
    fn blank_lines_are_skipped_rather_than_counted_as_rows() {
        let import = read("Name,Price\n\nRice,46.00\n\n");
        assert!(import.is_clean(), "{:?}", import.problems);
        assert_eq!(import.products.len(), 1);
    }

    #[test]
    fn tabs_work_as_well_as_commas() {
        // What a paste from a spreadsheet actually produces.
        let import = from_delimited("Name\tPrice\nRice\t46.00\n", '\t', BDT, 1500).expect("parses");
        assert!(import.is_clean());
        assert_eq!(import.products[0].price, Money::from_minor(4_600, BDT));
    }

    #[test]
    fn a_negative_price_is_refused() {
        let import = read("Name,Price\nRice,-46.00\n");
        assert_eq!(import.problems[0].column, "price");
    }

    #[test]
    fn a_currency_is_never_inferred_from_the_file() {
        // A column of bare numbers says nothing about what they are denominated in.
        let import =
            from_delimited("Name,Price\nRice,46.00\n", ',', Currency::Sar, 1500).expect("parses");
        assert_eq!(import.products[0].price.currency(), Currency::Sar);
    }
}
