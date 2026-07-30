//! Demo data, for looking at the thing.
//!
//! **Debug builds only**, like the dev bridge, and for the same reason: this appends real events to
//! a real log. A release binary that could conjure a day's trading into a shop's books is not a
//! feature, it is a defect waiting for someone to click it.
//!
//! Two markets, because the whole product claim is that one codebase serves both and the only way
//! to see whether that is true is to look at them side by side. Bangladesh is a Mushak-registered
//! grocery in taka with a counter scale; the Gulf is a ZATCA-registered café in riyals with tables
//! and prep stations. Between them they exercise every screen.
//!
//! Nothing here is idempotent and nothing pretends to be. Seeding twice gives two catalogues,
//! because the log is append-only and a "seed" that quietly deleted things would be the one piece
//! of code in this program that destroys records.

use sahl_core::catalogue::{CatalogueEvent, ProductDetails, Unit};
use sahl_core::floor::{FloorEvent, TableDetails};
use sahl_core::inventory::InventoryEvent;
use sahl_core::kitchen::Station;
use sahl_core::money::{Currency, Money, Rate};
use sahl_core::outlet::{FiscalRegime, OutletEvent, OutletSettings, Profile};
use sahl_core::quantity::Quantity;
use sahl_core::sale::{SaleEvent, TenderMethod, VoidReason};
use sahl_core::scale::{Embedded, ScaleFormat};
use sahl_core::shift::ShiftEvent;
use sahl_core::staff::{ApprovalPolicy, Role, StaffEvent, pin};
use sahl_core::tax::{Discount, PricingMode};
use sahl_core::time::Timestamp;
use uuid::Uuid;

use crate::terminal::{Terminal, TerminalError};

/// Which shop to conjure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Market {
    /// A Dhaka grocery: taka, Mushak 6.3, a counter scale printing weight labels.
    Bangladesh,
    /// A Riyadh café: riyals, ZATCA Phase 1, tables and prep stations.
    Gulf,
}

impl Market {
    /// Parse the label the settings screen sends.
    ///
    /// # Errors
    /// [`TerminalError::Denied`] for anything else, rather than defaulting to a market — seeding
    /// the wrong country's tax setup would be discovered on a challan.
    pub fn from_label(label: &str) -> Result<Self, TerminalError> {
        match label {
            "bangladesh" => Ok(Self::Bangladesh),
            "gulf" => Ok(Self::Gulf),
            _ => Err(TerminalError::Denied),
        }
    }
}

/// Everyone's PIN in the demo data.
///
/// One PIN for all four accounts because the point is to click through screens, not to remember
/// four numbers. It is printed on the settings screen beside the button for the same reason.
///
/// Not 1234: the domain refuses a guessable PIN, and demo data that had to bypass its own rules
/// would be demonstrating something other than the product.
pub const DEMO_PIN: &str = "8317";

/// Fill an empty till with a shop.
///
/// # Errors
/// [`TerminalError`] if any event is refused or cannot be written. Nothing is rolled back — the log
/// is append-only, so a half-seeded till is exactly as recoverable as any other partial day.
pub fn seed(till: &mut Terminal, market: Market, now: Timestamp) -> Result<(), TerminalError> {
    let mut clock = Clock::new(now);

    for (index, (name, role)) in people().iter().enumerate() {
        till.record_staff(
            &StaffEvent::Enrolled {
                staff_id: staff_id(index),
                name: (*name).to_owned(),
                role: *role,
                // Salted per person, so two accounts with the same PIN do not share a hash.
                pin_hash: pin::hash(DEMO_PIN, &salt(name))
                    .map_err(sahl_core::staff::DirectoryError::Pin)?,
                at: clock.tick(),
                enrolled_by: Uuid::nil(),
            },
            Uuid::now_v7(),
            clock.now(),
        )?;
    }

    till.record_outlet(
        &OutletEvent::Configured {
            outlet_id: till.identity().outlet_id,
            settings: settings(market),
            at: clock.tick(),
            configured_by: staff_id(0),
        },
        Uuid::now_v7(),
        clock.now(),
    )?;

    for (index, details) in products(market).into_iter().enumerate() {
        till.record_catalogue(
            &CatalogueEvent::ProductAdded {
                product_id: product_id(market, index),
                details,
                at: clock.tick(),
                added_by: staff_id(0),
            },
            Uuid::now_v7(),
            clock.now(),
        )?;
    }

    for (index, details) in tables(market).into_iter().enumerate() {
        till.record_floor(
            &FloorEvent::TableAdded {
                table_id: table_id(index),
                details,
                at: clock.tick(),
                added_by: staff_id(0),
            },
            Uuid::now_v7(),
            clock.now(),
        )?;
    }

    stock(till, market, &mut clock)?;
    trade(till, market, &mut clock)?;

    Ok(())
}

/// Put things on the shelf, so the stock screen has something to say.
fn stock(till: &mut Terminal, market: Market, clock: &mut Clock) -> Result<(), TerminalError> {
    let currency = settings(market).currency;

    for (index, details) in products(market).into_iter().enumerate() {
        // Cost is roughly two-thirds of the shelf price. Not a real margin for any of these
        // products — it exists so the stock screen has a number, not so anybody plans on it.
        let unit_cost = details
            .price
            .mul_ratio(2, 3, sahl_core::money::Rounding::HalfUp)
            .unwrap_or(Money::from_minor(0, currency));

        till.record_stock(
            &InventoryEvent::BatchReceived {
                batch_id: batch_id(market, index),
                product_id: product_id(market, index),
                lot: None,
                expires_at: None,
                quantity: Quantity::from_milli(40_000),
                unit_cost,
                supplier: Some(match market {
                    Market::Bangladesh => "Dhaka Wholesale".to_owned(),
                    Market::Gulf => "Olaya Supply Co.".to_owned(),
                }),
                at: clock.tick(),
                received_by: staff_id(1),
            },
            Uuid::now_v7(),
            clock.now(),
        )?;
    }

    Ok(())
}

/// A day's trading.
///
/// Enough sales that the per-cashier comparisons mean something — the anomaly scan ignores anybody
/// with fewer than twenty, and demo data below that threshold would show an empty feed and teach
/// nothing about it.
///
/// **Ruma voids noticeably more than Nasrin, deliberately.** The feed exists to surface exactly
/// that shape, and a seed where everybody behaved identically would demonstrate a working feature
/// by showing nothing at all. It is a pattern to ask about, not a verdict — which is the point the
/// screen makes too.
fn trade(till: &mut Terminal, market: Market, clock: &mut Clock) -> Result<(), TerminalError> {
    let outlet = settings(market);
    let currency = outlet.currency;
    let regime = outlet.regime.label().to_owned();
    let catalogue = products(market);

    till.record_shift(
        &ShiftEvent::Opened {
            shift_id: Uuid::from_u128(0x5EED_0000_0000_4000),
            opened_by: staff_id(2),
            currency,
            opening_float: Money::from_minor(50_000, currency),
            at: clock.tick(),
        },
        Uuid::now_v7(),
        clock.now(),
    )?;

    let mut rng = Lcg::new(0x5EED_5EED);

    for ticket in 0_u32..52 {
        // Alternating, so both cashiers clear the twenty-sale floor the scan needs.
        let cashier = if ticket % 2 == 0 { 2 } else { 3 };
        let sale = Uuid::from_u128(0x5EED_0000_0001_0000_u128.saturating_add(u128::from(ticket)));

        till.record(
            &SaleEvent::Opened {
                sale_id: sale,
                opened_by: staff_id(cashier),
                currency,
                pricing_mode: PricingMode::TaxInclusive,
                rounding: sahl_core::money::Rounding::HalfUp,
            },
            Uuid::now_v7(),
            clock.tick(),
        )?;

        let line_count = rng.below(4).saturating_add(1);
        let mut lines = Vec::new();
        for slot in 0..line_count {
            let pick = usize::try_from(rng.below(u32::try_from(catalogue.len()).unwrap_or(1)))
                .unwrap_or_default();
            let Some(product) = catalogue.get(pick) else {
                continue;
            };
            let line = Uuid::from_u128(
                0x5EED_0000_0002_0000_u128
                    .saturating_add(u128::from(ticket).saturating_mul(16))
                    .saturating_add(u128::from(slot)),
            );

            till.record(
                &SaleEvent::LineAdded {
                    sale_id: sale,
                    line_id: line,
                    product_id: product_id(market, pick),
                    name: product.name.clone(),
                    unit_price: product.price,
                    quantity: Quantity::from_milli(if product.unit.is_divisible() {
                        250_i64.saturating_add(i64::from(rng.below(8)).saturating_mul(250))
                    } else {
                        1_000_i64.saturating_add(i64::from(rng.below(2)).saturating_mul(1_000))
                    }),
                    tax_class: product.tax_class,
                    modifiers: Vec::new(),
                },
                Uuid::now_v7(),
                clock.tick(),
            )?;
            lines.push(line);
        }

        // Ruma strikes off roughly one line in three; Nasrin one in twelve. Both void — a seed
        // where only one person ever did would demonstrate the detector noticing the *only* one,
        // which is its weakest branch, rather than the rate comparison that is the actual feature.
        let voids_often = cashier == 2;
        if lines.len() > 1 && rng.below(if voids_often { 3 } else { 12 }) == 0 {
            let victim = lines.remove(0);
            till.record(
                &SaleEvent::LineVoided {
                    sale_id: sale,
                    line_id: victim,
                    reason: VoidReason::CustomerChanged,
                    // Inside the void limit, so the cashier is their own approver — which is what
                    // the threshold is for and what the feed must not mistake for a bypass.
                    authorized_by: staff_id(cashier),
                },
                Uuid::now_v7(),
                clock.tick(),
            )?;
        }

        if lines.is_empty() {
            continue;
        }

        // Every eleventh ticket gets a small discount, inside what a cashier may give.
        if ticket % 11 == 0 {
            till.record(
                &SaleEvent::OrderDiscounted {
                    sale_id: sale,
                    discount: Discount::Amount {
                        amount: Money::from_minor(200, currency),
                    },
                    authorized_by: staff_id(cashier),
                },
                Uuid::now_v7(),
                clock.tick(),
            )?;
        }

        // Two tickets are left open, because a real café always has some.
        if market == Market::Gulf && ticket >= 50 {
            continue;
        }

        let total = till.sale(sale)?.totals()?.total;
        till.record(
            &SaleEvent::TenderRecorded {
                sale_id: sale,
                tender_id: Uuid::now_v7(),
                method: if ticket % 3 == 0 {
                    TenderMethod::Card
                } else {
                    TenderMethod::Cash
                },
                amount: total,
                reference: None,
            },
            Uuid::now_v7(),
            clock.tick(),
        )?;

        till.complete_sale(
            &SaleEvent::Completed {
                sale_id: sale,
                total,
                change_given: Money::from_minor(0, currency),
                at: clock.tick(),
            },
            &regime,
            staff_id(cashier),
            clock.now(),
        )?;
    }

    Ok(())
}

/// A tiny linear congruential generator.
///
/// Not for security — for a basket that looks like a basket. Seeded by a constant so the demo shop
/// is the same shop every time; `rand` would make two seedings of "the same" data differ, and the
/// first thing anybody does with demo data is compare two screens showing it.
struct Lcg {
    state: u64,
}

impl Lcg {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn below(&mut self, bound: u32) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        if bound == 0 {
            return 0;
        }
        u32::try_from(self.state >> 33)
            .unwrap_or_default()
            .checked_rem(bound)
            .unwrap_or_default()
    }
}

/// A monotonic clock for the seed, so every event has its own instant.
///
/// Events sharing a timestamp is legal but makes a feed sorted by time arbitrary, and the first
/// thing anybody does with demo data is look at a feed.
struct Clock {
    at: Timestamp,
}

impl Clock {
    const fn new(at: Timestamp) -> Self {
        Self { at }
    }

    fn tick(&mut self) -> Timestamp {
        self.at = Timestamp::from_millis(self.at.millis().saturating_add(1_000));
        self.at
    }

    const fn now(&self) -> Timestamp {
        self.at
    }
}

fn people() -> [(&'static str, Role); 4] {
    [
        ("Karim Uddin", Role::Owner),
        ("Habib Rahman", Role::Manager),
        ("Ruma Akter", Role::Cashier),
        ("Nasrin Sultana", Role::Cashier),
    ]
}

fn settings(market: Market) -> OutletSettings {
    match market {
        Market::Bangladesh => OutletSettings {
            name: "Karim Store — Dhanmondi".to_owned(),
            profile: Profile::Grocery,
            currency: Currency::Bdt,
            timezone: "Asia/Dhaka".to_owned(),
            regime: FiscalRegime::BdMushak,
            tax_registration: Some("0031234567890".to_owned()),
            address: "12 Dhanmondi 27, Dhaka 1209".to_owned(),
            // The common grocery layout: prefix 20, five-digit item code, weight in grams. The
            // constructor validates, and a layout this file got wrong should not seed silently.
            scale: ScaleFormat::new("20", 5, Embedded::Weight, 5, 3, 0).ok(),
            approval: Some(ApprovalPolicy {
                discount_limit: Money::from_minor(5_000, Currency::Bdt),
                discount_rate_limit: Rate::from_basis_points(500),
                void_limit: Money::from_minor(20_000, Currency::Bdt),
            }),
        },
        Market::Gulf => OutletSettings {
            name: "Qahwa House — Al Olaya".to_owned(),
            profile: Profile::Cafe,
            currency: Currency::Sar,
            timezone: "Asia/Riyadh".to_owned(),
            regime: FiscalRegime::Zatca,
            tax_registration: Some("300000000000003".to_owned()),
            address: "King Fahd Road, Al Olaya, Riyadh 12211".to_owned(),
            scale: None,
            approval: Some(ApprovalPolicy {
                discount_limit: Money::from_minor(1_000, Currency::Sar),
                discount_rate_limit: Rate::from_basis_points(1000),
                void_limit: Money::from_minor(5_000, Currency::Sar),
            }),
        },
    }
}

/// What each shop sells.
///
/// Prices are tax-inclusive shelf prices, which is how both markets quote them. The tax engine
/// pulls the VAT back out, and seeing that happen on a real receipt is half the point of looking.
fn products(market: Market) -> Vec<ProductDetails> {
    match market {
        Market::Bangladesh => vec![
            // Item code 12345 matches the scale layout above, so a weight label scans.
            grocery(
                "Rice, loose",
                "12345",
                4_600,
                Unit::Kilogram,
                "Staples",
                1500,
            ),
            grocery(
                "Lentils, masoor",
                "12346",
                14_000,
                Unit::Kilogram,
                "Staples",
                1500,
            ),
            grocery("Sugar", "12347", 12_500, Unit::Kilogram, "Staples", 1500),
            // Zero-rated rather than exempt: a shop reclaims input VAT on these, and the two are
            // legally different however identical the receipt looks.
            zero_rated("Fresh milk 1L", "8901001", 9_000, Unit::Litre, "Dairy"),
            zero_rated("Eggs, dozen", "8901002", 16_500, Unit::Pack, "Dairy"),
            standard("Soap bar", "8901003", 5_500, Unit::Piece, "Household", 1500),
            standard(
                "Cooking oil 5L",
                "8901004",
                89_000,
                Unit::Piece,
                "Staples",
                1500,
            ),
            standard(
                "Tea, 400g",
                "8901005",
                32_000,
                Unit::Pack,
                "Beverages",
                1500,
            ),
        ],
        Market::Gulf => vec![
            cafe("Espresso", "9001", 1_200, "Coffee", Station::Counter),
            cafe("Flat white", "9002", 1_800, "Coffee", Station::Counter),
            cafe("Karak chai", "9003", 800, "Coffee", Station::Counter),
            cafe("Fresh orange juice", "9004", 2_200, "Cold", Station::Bar),
            cafe("Club sandwich", "9005", 3_500, "Kitchen", Station::Kitchen),
            cafe("Shakshuka", "9006", 3_200, "Kitchen", Station::Kitchen),
            cafe("Date cake", "9007", 1_500, "Pastry", Station::Pass),
            // No station: it comes off a shelf, so it needs no preparation and no ticket.
            ProductDetails {
                name: "Bottled water".to_owned(),
                sku: Some("9008".to_owned()),
                barcodes: vec!["9008".to_owned()],
                price: Money::from_minor(200, Currency::Sar),
                unit: Unit::Piece,
                tax_class: sahl_core::tax::TaxClass::standard(1500),
                category: Some("Cold".to_owned()),
                station: None,
                option_groups: Vec::new(),
            },
        ],
    }
}

fn grocery(
    name: &str,
    barcode: &str,
    minor: i64,
    unit: Unit,
    category: &str,
    basis_points: i32,
) -> ProductDetails {
    standard(name, barcode, minor, unit, category, basis_points)
}

fn standard(
    name: &str,
    barcode: &str,
    minor: i64,
    unit: Unit,
    category: &str,
    basis_points: i32,
) -> ProductDetails {
    ProductDetails {
        name: name.to_owned(),
        sku: Some(barcode.to_owned()),
        barcodes: vec![barcode.to_owned()],
        price: Money::from_minor(minor, Currency::Bdt),
        unit,
        tax_class: sahl_core::tax::TaxClass::standard(basis_points),
        category: Some(category.to_owned()),
        station: None,
        option_groups: Vec::new(),
    }
}

fn zero_rated(name: &str, barcode: &str, minor: i64, unit: Unit, category: &str) -> ProductDetails {
    ProductDetails {
        tax_class: sahl_core::tax::TaxClass::ZeroRated,
        ..standard(name, barcode, minor, unit, category, 0)
    }
}

fn cafe(name: &str, barcode: &str, minor: i64, category: &str, station: Station) -> ProductDetails {
    ProductDetails {
        price: Money::from_minor(minor, Currency::Sar),
        station: Some(station),
        ..standard(name, barcode, minor, Unit::Piece, category, 1500)
    }
}

/// The floor. Empty for a grocery, which has a counter rather than tables.
fn tables(market: Market) -> Vec<TableDetails> {
    match market {
        Market::Bangladesh => Vec::new(),
        Market::Gulf => vec![
            table("1", "Window", 2),
            table("2", "Window", 2),
            table("3", "Window", 4),
            table("10", "Terrace", 4),
            table("11", "Terrace", 6),
            // Same label as table 1, in another section — legal, and the case that proves labels
            // are unique per section rather than per floor.
            table("1", "Majlis", 8),
        ],
    }
}

fn table(label: &str, section: &str, seats: u32) -> TableDetails {
    TableDetails {
        label: label.to_owned(),
        section: Some(section.to_owned()),
        seats,
    }
}

/// Stable ids, so seeding the same market twice is visibly the same shop rather than a new one.
fn staff_id(index: usize) -> Uuid {
    Uuid::from_u128(0x5EED_0000_0000_0001_u128.saturating_add(index as u128))
}

fn product_id(market: Market, index: usize) -> Uuid {
    let market_offset: u128 = match market {
        Market::Bangladesh => 0x1000,
        Market::Gulf => 0x2000,
    };
    Uuid::from_u128(
        0x5EED_0000_0000_0000_u128
            .saturating_add(market_offset)
            .saturating_add(index as u128),
    )
}

fn batch_id(market: Market, index: usize) -> Uuid {
    let market_offset: u128 = match market {
        Market::Bangladesh => 0x5000,
        Market::Gulf => 0x6000,
    };
    Uuid::from_u128(
        0x5EED_0000_0000_0000_u128
            .saturating_add(market_offset)
            .saturating_add(index as u128),
    )
}

fn table_id(index: usize) -> Uuid {
    Uuid::from_u128(0x5EED_0000_0000_3000_u128.saturating_add(index as u128))
}

/// A per-person salt, so two accounts sharing the demo PIN do not share a hash.
fn salt(name: &str) -> argon2::password_hash::SaltString {
    let padded = format!("{name}-sahl-demo-seed");
    argon2::password_hash::SaltString::encode_b64(padded.as_bytes()).unwrap_or_else(|_| {
        argon2::password_hash::SaltString::encode_b64(b"sahl-demo")
            .unwrap_or_else(|_| unreachable!())
    })
}
