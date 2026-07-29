use crate::money::{Currency, Money, Rate};
use crate::quantity::Quantity;

use super::class::TaxClass;
use super::error::TaxError;
use super::order::{OrderInput, PricingMode};
use super::totals::{LineTotals, OrderTotals, TaxGroup};

/// Calculate an order: line totals, VAT summary, and order aggregates.
///
/// The whole function is built around one rule: **every aggregate is the exact sum of its parts.**
/// Nothing is recomputed from a rounded subtotal. That is what keeps a printed invoice internally
/// consistent — a summary block that disagrees with the lines above it is the classic POS defect,
/// and it is what fiscal auditors in both target markets look for first.
///
/// Order of operations, which is itself a correctness decision:
/// 1. Line gross is `unit_price × quantity`, rounded once.
/// 2. Line discount resolves against that gross.
/// 3. The order discount resolves against the sum of discounted lines, then is **apportioned back
///    across those lines** by largest-remainder allocation. Apportioning rather than subtracting at
///    the end is what keeps per-line VAT correct on a mixed-rate basket: a flat ৳50 off a basket
///    holding both 15% and exempt goods must reduce each line's taxable base proportionally, or the
///    VAT owed is simply wrong.
/// 4. Tax is resolved per line according to [`PricingMode`].
///
/// # Errors
/// [`TaxError::EmptyOrder`], [`TaxError::LineCurrencyMismatch`], or [`TaxError::Money`] on overflow.
pub fn calculate(order: &OrderInput) -> Result<OrderTotals, TaxError> {
    if order.lines.is_empty() {
        return Err(TaxError::EmptyOrder);
    }

    let currency = order.currency;
    let rounding = order.rounding;

    let mut gross_amounts: Vec<Money> = Vec::with_capacity(order.lines.len());
    let mut discounts: Vec<Money> = Vec::with_capacity(order.lines.len());
    let mut bases: Vec<Money> = Vec::with_capacity(order.lines.len());

    for (index, line) in order.lines.iter().enumerate() {
        if line.unit_price.currency() != currency {
            return Err(TaxError::LineCurrencyMismatch {
                index,
                expected: currency,
                found: line.unit_price.currency(),
            });
        }

        let gross =
            line.unit_price
                .mul_ratio(line.quantity.milli(), Quantity::MILLI_PER_UNIT, rounding)?;
        let discount = line.discount.resolve(gross, rounding)?;
        let base = gross.checked_sub(discount)?;

        gross_amounts.push(gross);
        discounts.push(discount);
        bases.push(base);
    }

    let subtotal = Money::try_sum(bases.iter().copied(), currency)?;
    let order_discount = order.order_discount.resolve(subtotal, rounding)?;
    let apportioned = apportion(order_discount, &bases, currency)?;

    let mut lines: Vec<LineTotals> = Vec::with_capacity(order.lines.len());
    let mut groups: Vec<TaxGroup> = Vec::new();

    for (index, line) in order.lines.iter().enumerate() {
        let gross = *gross_amounts.get(index).ok_or(TaxError::EmptyOrder)?;
        let line_discount = *discounts.get(index).ok_or(TaxError::EmptyOrder)?;
        let base = *bases.get(index).ok_or(TaxError::EmptyOrder)?;
        let share = *apportioned.get(index).ok_or(TaxError::EmptyOrder)?;

        let taxable = base.checked_sub(share)?;
        let (net, tax) = resolve_tax(taxable, line.tax_class, order.pricing_mode, rounding)?;

        lines.push(LineTotals {
            gross,
            discount: line_discount.checked_add(share)?,
            net,
            tax,
            total: net.checked_add(tax)?,
            tax_class: line.tax_class,
        });

        accumulate_group(&mut groups, line.tax_class, net, tax)?;
    }

    groups.sort_by_key(|group| group.tax_class.sort_key());

    Ok(OrderTotals {
        gross: Money::try_sum(lines.iter().map(|line| line.gross), currency)?,
        discount: Money::try_sum(lines.iter().map(|line| line.discount), currency)?,
        net: Money::try_sum(lines.iter().map(|line| line.net), currency)?,
        tax: Money::try_sum(lines.iter().map(|line| line.tax), currency)?,
        total: Money::try_sum(lines.iter().map(|line| line.total), currency)?,
        tax_groups: groups,
        lines,
    })
}

/// Split `net` and `tax` out of a taxable amount according to the pricing mode.
///
/// The inclusive branch computes **tax first and subtracts**, rather than computing net first and
/// adding. Both are defensible on paper; only the first guarantees `net + tax == the original
/// amount` for every input. That guarantee is what makes a ৳100 shelf label ring up as ৳100 instead
/// of ৳99.99 — a one-paisa discrepancy that a merchant will notice on day one and never forgive.
fn resolve_tax(
    taxable: Money,
    tax_class: TaxClass,
    mode: PricingMode,
    rounding: crate::money::Rounding,
) -> Result<(Money, Money), TaxError> {
    let rate = tax_class.rate();

    if rate.is_zero() {
        return Ok((taxable, Money::zero(taxable.currency())));
    }

    match mode {
        PricingMode::TaxExclusive => {
            let tax = taxable.apply_rate(rate, rounding)?;
            Ok((taxable, tax))
        }
        PricingMode::TaxInclusive => {
            let basis_points = i64::from(rate.basis_points());
            let denominator = i64::from(Rate::BASIS_POINTS_PER_UNIT)
                .checked_add(basis_points)
                .ok_or(crate::money::MoneyError::Overflow)?;
            let tax = taxable.mul_ratio(basis_points, denominator, rounding)?;
            let net = taxable.checked_sub(tax)?;
            Ok((net, tax))
        }
    }
}

/// Spread an order-level discount across lines in proportion to their discounted value.
///
/// Delegates to [`Money::allocate`], so the shares sum to exactly the discount given — no cent is
/// created or destroyed by the apportionment itself.
fn apportion(amount: Money, bases: &[Money], currency: Currency) -> Result<Vec<Money>, TaxError> {
    if amount.is_zero() || bases.is_empty() {
        return Ok(vec![Money::zero(currency); bases.len()]);
    }

    let weights: Vec<u64> = bases
        .iter()
        .map(|base| base.minor().unsigned_abs())
        .collect();

    // Every line is zero-valued (a fully discounted or zero-price basket). There is no proportional
    // answer, so fall back to an even spread rather than failing the sale.
    if weights.iter().all(|weight| *weight == 0) {
        let even = vec![1u64; bases.len()];
        return Ok(amount.allocate(&even)?);
    }

    Ok(amount.allocate(&weights)?)
}

/// Accumulate a line into its VAT summary group, creating the group on first sight.
///
/// Uses a linear scan over a `Vec` rather than a `HashMap` deliberately: iteration order of a
/// `HashMap` is not stable across processes, and terminal and server must emit byte-identical
/// invoices. The group count is single digits, so the scan costs nothing.
fn accumulate_group(
    groups: &mut Vec<TaxGroup>,
    tax_class: TaxClass,
    net: Money,
    tax: Money,
) -> Result<(), TaxError> {
    if let Some(group) = groups.iter_mut().find(|group| group.tax_class == tax_class) {
        group.taxable_base = group.taxable_base.checked_add(net)?;
        group.tax = group.tax.checked_add(tax)?;
    } else {
        groups.push(TaxGroup {
            tax_class,
            taxable_base: net,
            tax,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::{Rate, Rounding};
    use crate::tax::discount::Discount;
    use crate::tax::order::LineInput;

    const BDT: Currency = Currency::Bdt;
    const VAT_15: TaxClass = TaxClass::standard(1500);
    const VAT_7_5: TaxClass = TaxClass::standard(750);

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, BDT)
    }

    fn one(unit_price: i64, tax_class: TaxClass) -> LineInput {
        LineInput::new(bdt(unit_price), Quantity::ONE, tax_class)
    }

    #[test]
    fn a_tax_inclusive_shelf_label_rings_up_at_exactly_that_price() {
        // The invariant a merchant checks on day one: a ৳100 packet must total ৳100.00, not 99.99.
        let order = OrderInput::new(BDT, vec![one(10_000, VAT_15)]);
        let totals = calculate(&order).expect("calculates");

        assert_eq!(totals.total, bdt(10_000));
        assert_eq!(totals.tax, bdt(1_304)); // 10000 × 1500/11500
        assert_eq!(totals.net, bdt(8_696));
        assert_eq!(
            totals.net.checked_add(totals.tax),
            Ok(bdt(10_000)),
            "net + tax must reconstruct the label price exactly"
        );
    }

    #[test]
    fn tax_exclusive_pricing_adds_on_top() {
        let order = OrderInput::new(BDT, vec![one(10_000, VAT_15)]).tax_exclusive();
        let totals = calculate(&order).expect("calculates");

        assert_eq!(totals.net, bdt(10_000));
        assert_eq!(totals.tax, bdt(1_500));
        assert_eq!(totals.total, bdt(11_500));
    }

    #[test]
    fn a_weighed_grocery_line_is_exact() {
        // 1.234 kg at ৳80.00/kg = ৳98.72.
        let line = LineInput::new(bdt(8_000), Quantity::from_milli(1_234), VAT_15);
        let order = OrderInput::new(BDT, vec![line]);
        let totals = calculate(&order).expect("calculates");

        assert_eq!(totals.gross, bdt(9_872));
        assert_eq!(totals.total, bdt(9_872));
    }

    #[test]
    fn zero_rated_and_exempt_stay_separate_on_the_summary() {
        // Arithmetically identical, legally distinct — the filing depends on this split.
        let order = OrderInput::new(
            BDT,
            vec![
                one(10_000, TaxClass::ZeroRated),
                one(5_000, TaxClass::Exempt),
            ],
        );
        let totals = calculate(&order).expect("calculates");

        assert_eq!(totals.tax_groups.len(), 2);
        assert_eq!(totals.tax, bdt(0));
        assert_eq!(totals.total, bdt(15_000));
    }

    #[test]
    fn a_mixed_rate_basket_groups_by_class_in_invoice_order() {
        let order = OrderInput::new(
            BDT,
            vec![
                one(10_000, VAT_15),
                one(20_000, TaxClass::Exempt),
                one(10_000, VAT_7_5),
                one(30_000, VAT_15),
            ],
        );
        let totals = calculate(&order).expect("calculates");

        let classes: Vec<TaxClass> = totals
            .tax_groups
            .iter()
            .map(|group| group.tax_class)
            .collect();
        assert_eq!(classes, vec![VAT_7_5, VAT_15, TaxClass::Exempt]);

        // The two 15% lines merged into one group.
        let standard = totals
            .tax_groups
            .iter()
            .find(|group| group.tax_class == VAT_15)
            .expect("15% group exists");
        assert_eq!(
            standard.taxable_base.checked_add(standard.tax),
            Ok(bdt(40_000))
        );
    }

    #[test]
    fn group_tax_sums_to_order_tax() {
        let order = OrderInput::new(
            BDT,
            vec![one(3_333, VAT_15), one(6_667, VAT_7_5), one(1_111, VAT_15)],
        );
        let totals = calculate(&order).expect("calculates");

        let summed = Money::try_sum(totals.tax_groups.iter().map(|group| group.tax), BDT);
        assert_eq!(summed, Ok(totals.tax));
    }

    #[test]
    fn an_order_discount_is_apportioned_so_mixed_rate_vat_stays_right() {
        // A flat ৳50 off a basket holding both taxable and exempt goods must reduce each line's
        // taxable base proportionally. Subtracting it at the end instead would overstate the VAT.
        let order = OrderInput::new(
            BDT,
            vec![one(10_000, VAT_15), one(10_000, TaxClass::Exempt)],
        )
        .with_order_discount(Discount::Amount { amount: bdt(5_000) });
        let totals = calculate(&order).expect("calculates");

        assert_eq!(totals.discount, bdt(5_000));
        assert_eq!(totals.total, bdt(15_000));

        // Equal bases, so the discount splits evenly.
        let shares: Vec<Money> = totals.lines.iter().map(|line| line.discount).collect();
        assert_eq!(shares, vec![bdt(2_500), bdt(2_500)]);

        // The exempt line contributes no tax regardless of the discount.
        let exempt = totals
            .lines
            .iter()
            .find(|line| line.tax_class == TaxClass::Exempt)
            .expect("exempt line");
        assert!(exempt.tax.is_zero());
    }

    #[test]
    fn an_order_discount_that_does_not_divide_evenly_still_sums_exactly() {
        let order = OrderInput::new(
            BDT,
            vec![one(3_333, VAT_15), one(3_333, VAT_15), one(3_334, VAT_15)],
        )
        .with_order_discount(Discount::Amount { amount: bdt(1_000) });
        let totals = calculate(&order).expect("calculates");

        let apportioned = Money::try_sum(totals.lines.iter().map(|line| line.discount), BDT);
        assert_eq!(apportioned, Ok(bdt(1_000)));
    }

    #[test]
    fn line_and_order_discounts_compose() {
        let line = one(10_000, VAT_15).with_discount(Discount::Percentage {
            rate: Rate::from_basis_points(1000),
        });
        let order = OrderInput::new(BDT, vec![line])
            .with_order_discount(Discount::Amount { amount: bdt(1_000) });
        let totals = calculate(&order).expect("calculates");

        // ৳100 less 10% is ৳90, less a flat ৳10 is ৳80.
        assert_eq!(totals.gross, bdt(10_000));
        assert_eq!(totals.discount, bdt(2_000));
        assert_eq!(totals.total, bdt(8_000));
    }

    #[test]
    fn a_fully_discounted_basket_does_not_fail_the_sale() {
        // Every line at zero value: there is no proportional split, but the sale must still ring.
        let order = OrderInput::new(BDT, vec![one(0, VAT_15), one(0, VAT_15)])
            .with_order_discount(Discount::Amount { amount: bdt(500) });
        let totals = calculate(&order).expect("calculates");

        assert_eq!(totals.total, bdt(0));
    }

    #[test]
    fn aggregates_are_the_exact_sum_of_their_lines() {
        let order = OrderInput::new(
            BDT,
            vec![one(1_999, VAT_15), one(2_499, VAT_7_5), one(777, VAT_15)],
        );
        let totals = calculate(&order).expect("calculates");

        assert_eq!(
            Money::try_sum(totals.lines.iter().map(|line| line.net), BDT),
            Ok(totals.net)
        );
        assert_eq!(
            Money::try_sum(totals.lines.iter().map(|line| line.tax), BDT),
            Ok(totals.tax)
        );
        assert_eq!(totals.net.checked_add(totals.tax), Ok(totals.total));
    }

    #[test]
    fn a_line_in_the_wrong_currency_is_refused_with_its_position() {
        let order = OrderInput::new(
            BDT,
            vec![
                one(1_000, VAT_15),
                LineInput::new(
                    Money::from_minor(1_000, Currency::Sar),
                    Quantity::ONE,
                    VAT_15,
                ),
            ],
        );

        assert_eq!(
            calculate(&order),
            Err(TaxError::LineCurrencyMismatch {
                index: 1,
                expected: Currency::Bdt,
                found: Currency::Sar,
            })
        );
    }

    #[test]
    fn an_empty_order_is_refused() {
        let order = OrderInput::new(BDT, vec![]);
        assert_eq!(calculate(&order), Err(TaxError::EmptyOrder));
    }

    #[test]
    fn a_return_line_mirrors_its_sale() {
        let sale = OrderInput::new(BDT, vec![one(9_999, VAT_15)]);
        let refund = OrderInput::new(
            BDT,
            vec![LineInput::new(
                bdt(9_999),
                Quantity::from_milli(-1_000),
                VAT_15,
            )],
        );

        let sold = calculate(&sale).expect("calculates");
        let returned = calculate(&refund).expect("calculates");

        assert_eq!(sold.total.checked_neg(), Ok(returned.total));
        assert_eq!(sold.tax.checked_neg(), Ok(returned.tax));
        assert_eq!(sold.net.checked_neg(), Ok(returned.net));
    }

    #[test]
    fn rounding_mode_is_honoured() {
        // A base whose 15% inclusive tax lands exactly on a half-minor-unit boundary.
        let mut order = OrderInput::new(BDT, vec![one(115, VAT_15)]);
        order.rounding = Rounding::HalfUp;
        let up = calculate(&order).expect("calculates");

        order.rounding = Rounding::TowardZero;
        let down = calculate(&order).expect("calculates");

        assert!(up.tax.minor() >= down.tax.minor());
        // Whichever way it rounds, the label price is preserved.
        assert_eq!(up.total, bdt(115));
        assert_eq!(down.total, bdt(115));
    }
}
