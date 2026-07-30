//! A day, totalled.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::{Currency, Money};
use crate::quantity::Quantity;
use crate::sale::{Sale, SaleError, TenderMethod};

/// One cashier's part of the day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CashierRow {
    pub staff_id: Uuid,
    pub sales: usize,
    pub takings: Money,
    pub discount: Money,
    /// Lines struck off. The count, not the value — a void's value is what the line *would* have
    /// been, which is not money that moved.
    pub voids: usize,
}

/// What was taken by each method.
///
/// Cash is the row that has to reconcile against a drawer, which is why the split exists at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentRow {
    pub method: TenderMethod,
    pub count: usize,
    /// What the customer handed over, less any change given back.
    pub taken: Money,
}

/// One product's part of the day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductRow {
    pub product_id: Uuid,
    pub name: String,
    /// Thousandths, so a weighed line counts as what it weighed.
    pub quantity_milli: i64,
    /// Including tax, because that is what the customer paid and what the shelf says.
    pub revenue: Money,
}

/// A day's trading.
///
/// Built from completed sales only. An open ticket has not finished being what it is, and counting
/// one would make the figure move every time somebody added a line to a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Day {
    pub currency: Currency,
    pub sales: usize,
    /// What customers paid, tax included.
    pub takings: Money,
    /// The taxable base — takings less tax.
    pub net: Money,
    pub tax: Money,
    /// Given away, line-level and order-level together.
    pub discount: Money,
    /// Takings divided by sales, or zero on a day with none.
    pub average_sale: Money,
    pub voids: usize,
    pub by_cashier: Vec<CashierRow>,
    pub by_payment: Vec<PaymentRow>,
    /// Busiest first, then by name so two products that sold the same are ordered stably.
    pub by_product: Vec<ProductRow>,
}

impl Day {
    /// Total a set of completed sales.
    ///
    /// # Errors
    /// [`SaleError`] if a sale's own totals cannot be computed — which means it is malformed, not
    /// that the report failed.
    pub fn of(sales: &[&Sale], currency: Currency) -> Result<Self, SaleError> {
        let mut day = Self::empty(currency);

        let mut cashiers: BTreeMap<Uuid, CashierRow> = BTreeMap::new();
        // Keyed by the method itself, which is already ordered — a string key would be a second
        // naming of something the domain already names.
        let mut payments: BTreeMap<TenderMethod, PaymentRow> = BTreeMap::new();
        let mut products: BTreeMap<Uuid, ProductRow> = BTreeMap::new();

        for sale in sales {
            // Only settled sales count. A ticket still open has not finished being what it is.
            let Some(total) = sale.settled_total() else {
                continue;
            };
            let totals = sale.totals()?;

            day.sales = day.sales.saturating_add(1);
            day.takings = day.takings.checked_add(total)?;
            day.net = day.net.checked_add(totals.net)?;
            day.tax = day.tax.checked_add(totals.tax)?;
            day.discount = day.discount.checked_add(totals.discount)?;
            day.voids = day.voids.saturating_add(sale.void_count());

            let row = cashiers
                .entry(sale.opened_by())
                .or_insert_with(|| CashierRow {
                    staff_id: sale.opened_by(),
                    sales: 0,
                    takings: Money::zero(currency),
                    discount: Money::zero(currency),
                    voids: 0,
                });
            row.sales = row.sales.saturating_add(1);
            row.takings = row.takings.checked_add(total)?;
            row.discount = row.discount.checked_add(totals.discount)?;
            row.voids = row.voids.saturating_add(sale.void_count());

            // Change comes off the cash row, not off the total: the drawer holds the difference,
            // and a cash figure that ignored change would never reconcile against a count.
            let change = sale.change_given().unwrap_or_else(|| Money::zero(currency));
            for tender in sale.tenders() {
                let entry = payments.entry(tender.method).or_insert_with(|| PaymentRow {
                    method: tender.method,
                    count: 0,
                    taken: Money::zero(currency),
                });
                entry.count = entry.count.saturating_add(1);
                entry.taken = entry.taken.checked_add(tender.amount)?;
                if tender.method.accepts_overtender() && !change.is_zero() {
                    entry.taken = entry.taken.checked_sub(change)?;
                }
            }

            for (line, computed) in sale.lines().iter().zip(totals.lines.iter()) {
                if !line.is_active() {
                    continue;
                }
                let entry = products
                    .entry(line.product_id)
                    .or_insert_with(|| ProductRow {
                        product_id: line.product_id,
                        name: line.name.clone(),
                        quantity_milli: 0,
                        revenue: Money::zero(currency),
                    });
                entry.quantity_milli = entry.quantity_milli.saturating_add(line.quantity.milli());
                entry.revenue = entry.revenue.checked_add(computed.total)?;
            }
        }

        day.average_sale = if day.sales == 0 {
            Money::zero(currency)
        } else {
            day.takings
                .allocate(&vec![1_u64; day.sales])?
                .first()
                .copied()
                .unwrap_or_else(|| Money::zero(currency))
        };

        day.by_cashier = cashiers.into_values().collect();
        day.by_payment = payments.into_values().collect();

        let mut by_product: Vec<ProductRow> = products.into_values().collect();
        // Busiest first, then by name: two products that took the same must not swap places
        // between one reading and the next.
        by_product.sort_by(|left, right| {
            right
                .revenue
                .minor()
                .cmp(&left.revenue.minor())
                .then(left.name.cmp(&right.name))
        });
        day.by_product = by_product;

        Ok(day)
    }

    /// A day on which nothing happened. A real answer, not a missing one.
    #[must_use]
    pub fn empty(currency: Currency) -> Self {
        Self {
            currency,
            sales: 0,
            takings: Money::zero(currency),
            net: Money::zero(currency),
            tax: Money::zero(currency),
            discount: Money::zero(currency),
            average_sale: Money::zero(currency),
            voids: 0,
            by_cashier: Vec::new(),
            by_payment: Vec::new(),
            by_product: Vec::new(),
        }
    }

    /// Units sold, for a caller that wants one figure.
    #[must_use]
    pub fn units(&self) -> Quantity {
        Quantity::from_milli(
            self.by_product
                .iter()
                .fold(0_i64, |sum, row| sum.saturating_add(row.quantity_milli)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Rounding;
    use crate::sale::{SaleEvent, VoidReason};
    use crate::tax::{Discount, PricingMode, TaxClass};

    const BDT: Currency = Currency::Bdt;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(n: i64) -> crate::time::Timestamp {
        crate::time::Timestamp::from_millis(1_753_000_000_000 + n)
    }

    fn bdt(minor: i64) -> Money {
        Money::zero(BDT)
            .checked_add(Money::from_minor(minor, BDT))
            .expect("adds")
    }

    /// Open a sale and add one line at `minor`.
    fn opened_with_line(base: u128, cashier: u128, minor: i64) -> Vec<SaleEvent> {
        vec![
            SaleEvent::Opened {
                sale_id: id(base),
                opened_by: id(cashier),
                currency: BDT,
                pricing_mode: PricingMode::TaxInclusive,
                rounding: Rounding::HalfUp,
            },
            SaleEvent::LineAdded {
                sale_id: id(base),
                line_id: id(base + 1),
                product_id: id(7),
                name: "Rice".to_owned(),
                unit_price: bdt(minor),
                quantity: Quantity::ONE,
                tax_class: TaxClass::standard(1500),
                modifiers: Vec::new(),
            },
        ]
    }

    /// Settle a sale for `total`, paid by `method`.
    ///
    /// Separate from opening it because the domain refuses anything applied after completion — as
    /// it should. A discount or a void has to happen while the sale is still open, which is also
    /// the only order in which either could really happen.
    fn settle(base: u128, mut events: Vec<SaleEvent>, total: i64, method: TenderMethod) -> Sale {
        events.push(SaleEvent::TenderRecorded {
            sale_id: id(base),
            tender_id: id(base + 2),
            method,
            amount: bdt(total),
            reference: None,
        });
        events.push(SaleEvent::Completed {
            sale_id: id(base),
            total: bdt(total),
            change_given: bdt(0),
            at: at(0),
        });
        Sale::replay(&events).expect("valid")
    }

    /// The ordinary case: one line, paid in full.
    fn settled(base: u128, cashier: u128, minor: i64, method: TenderMethod) -> Sale {
        settle(base, opened_with_line(base, cashier, minor), minor, method)
    }

    #[test]
    fn a_day_with_nothing_on_it_is_an_answer_not_a_gap() {
        let day = Day::of(&[], BDT).expect("totals");

        assert_eq!(day.sales, 0);
        assert_eq!(day.takings, bdt(0));
        assert_eq!(day.average_sale, bdt(0), "not a division by zero");
    }

    #[test]
    fn takings_are_what_customers_paid() {
        let first = settled(0x100, 0xCA, 11_500, TenderMethod::Cash);
        let second = settled(0x200, 0xCA, 23_000, TenderMethod::Cash);
        let day = Day::of(&[&first, &second], BDT).expect("totals");

        assert_eq!(day.sales, 2);
        assert_eq!(day.takings, bdt(34_500));
        assert_eq!(
            day.tax,
            bdt(4_500),
            "15% of the taxable base, not of the total"
        );
        assert_eq!(day.net, bdt(30_000));
        assert_eq!(day.takings, day.net.checked_add(day.tax).expect("adds"));
    }

    #[test]
    fn an_open_ticket_is_not_part_of_the_day() {
        // Counting one would make the figure move every time somebody added a line to a table.
        let open = Sale::replay(&[SaleEvent::Opened {
            sale_id: id(0x300),
            opened_by: id(0xCA),
            currency: BDT,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        }])
        .expect("valid");
        let done = settled(0x100, 0xCA, 11_500, TenderMethod::Cash);

        let day = Day::of(&[&open, &done], BDT).expect("totals");
        assert_eq!(day.sales, 1);
    }

    #[test]
    fn the_day_is_split_by_who_rang_it() {
        let ruma = settled(0x100, 0xCA, 10_000, TenderMethod::Cash);
        let nasrin = settled(0x200, 0xCB, 30_000, TenderMethod::Cash);
        let day = Day::of(&[&ruma, &nasrin], BDT).expect("totals");

        assert_eq!(day.by_cashier.len(), 2);
        let hers = day
            .by_cashier
            .iter()
            .find(|row| row.staff_id == id(0xCB))
            .expect("present");
        assert_eq!(hers.takings, bdt(30_000));
        assert_eq!(hers.sales, 1);
    }

    #[test]
    fn cash_and_card_are_counted_apart() {
        // The cash row is the one that has to reconcile against a drawer, which is the whole
        // reason for the split.
        let cash = settled(0x100, 0xCA, 10_000, TenderMethod::Cash);
        let card = settled(0x200, 0xCA, 30_000, TenderMethod::Card);
        let day = Day::of(&[&cash, &card], BDT).expect("totals");

        let taken = |method: TenderMethod| {
            day.by_payment
                .iter()
                .find(|row| row.method == method)
                .map(|row| row.taken)
        };
        assert_eq!(taken(TenderMethod::Cash), Some(bdt(10_000)));
        assert_eq!(taken(TenderMethod::Card), Some(bdt(30_000)));
    }

    #[test]
    fn change_comes_off_the_cash_row() {
        // The drawer holds the difference. A cash figure that ignored change would never
        // reconcile against a count.
        let sale = Sale::replay(&[
            SaleEvent::Opened {
                sale_id: id(0x100),
                opened_by: id(0xCA),
                currency: BDT,
                pricing_mode: PricingMode::TaxInclusive,
                rounding: Rounding::HalfUp,
            },
            SaleEvent::LineAdded {
                sale_id: id(0x100),
                line_id: id(0x101),
                product_id: id(7),
                name: "Rice".to_owned(),
                unit_price: bdt(11_500),
                quantity: Quantity::ONE,
                tax_class: TaxClass::standard(1500),
                modifiers: Vec::new(),
            },
            SaleEvent::TenderRecorded {
                sale_id: id(0x100),
                tender_id: id(0x102),
                method: TenderMethod::Cash,
                amount: bdt(20_000),
                reference: None,
            },
            SaleEvent::Completed {
                sale_id: id(0x100),
                total: bdt(11_500),
                change_given: bdt(8_500),
                at: at(0),
            },
        ])
        .expect("valid");

        let day = Day::of(&[&sale], BDT).expect("totals");
        assert_eq!(
            day.by_payment[0].taken,
            bdt(11_500),
            "handed over less change"
        );
        assert_eq!(day.takings, bdt(11_500));
    }

    #[test]
    fn a_voided_line_sells_nothing() {
        let mut events = opened_with_line(0x100, 0xCA, 11_500);
        events.push(SaleEvent::LineAdded {
            sale_id: id(0x100),
            line_id: id(0x109),
            product_id: id(9),
            name: "Oil".to_owned(),
            unit_price: bdt(5_000),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            modifiers: Vec::new(),
        });
        events.push(SaleEvent::LineVoided {
            sale_id: id(0x100),
            line_id: id(0x109),
            reason: VoidReason::Mistake,
            authorized_by: id(0x11A),
        });
        let sale = settle(0x100, events, 11_500, TenderMethod::Cash);

        let day = Day::of(&[&sale], BDT).expect("totals");
        assert_eq!(day.voids, 1);
        assert_eq!(day.by_product.len(), 1, "the struck-off line sold nothing");
        assert_eq!(day.by_product[0].name, "Rice");
    }

    #[test]
    fn products_are_ranked_by_what_they_took() {
        let cheap = settled(0x100, 0xCA, 1_000, TenderMethod::Cash);
        let mut events = opened_with_line(0x200, 0xCA, 50_000);
        events.push(SaleEvent::LineAdded {
            sale_id: id(0x200),
            line_id: id(0x203),
            product_id: id(9),
            name: "Oil".to_owned(),
            unit_price: bdt(90_000),
            quantity: Quantity::ONE,
            tax_class: TaxClass::standard(1500),
            modifiers: Vec::new(),
        });
        let dear = settle(0x200, events, 140_000, TenderMethod::Cash);

        let day = Day::of(&[&cheap, &dear], BDT).expect("totals");
        assert_eq!(day.by_product[0].name, "Oil", "busiest first");
    }

    #[test]
    fn the_average_sale_is_not_a_division_by_zero() {
        let one = settled(0x100, 0xCA, 10_000, TenderMethod::Cash);
        let two = settled(0x200, 0xCA, 20_001, TenderMethod::Cash);
        let day = Day::of(&[&one, &two], BDT).expect("totals");

        assert_eq!(day.average_sale, bdt(15_001), "the remainder lands first");
    }

    #[test]
    fn a_discount_shows_up_against_the_person_who_gave_it() {
        let mut events = opened_with_line(0x100, 0xCA, 11_500);
        events.push(SaleEvent::OrderDiscounted {
            sale_id: id(0x100),
            discount: Discount::Amount { amount: bdt(500) },
            authorized_by: id(0x11A),
        });
        let sale = settle(0x100, events, 11_000, TenderMethod::Cash);

        let day = Day::of(&[&sale], BDT).expect("totals");
        assert_eq!(day.discount, bdt(500));
        assert_eq!(day.by_cashier[0].discount, bdt(500));
    }
}
