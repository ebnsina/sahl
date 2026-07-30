//! Reading a day's activity for things worth asking about.

use std::collections::BTreeMap;

use uuid::Uuid;

use crate::money::{Currency, Money};
use crate::sale::{Sale, SaleError};
use crate::staff::{AuditEntry, Role, Severity, unapproved};

use super::finding::{Finding, Subject, ranked};

/// How readily the scan speaks up.
///
/// These are dials on noise, not judgements about behaviour — the comparisons themselves are all
/// against the outlet's own numbers or a limit the owner already set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sensitivity {
    /// How many completed sales someone needs before their rate is compared to anyone else's.
    ///
    /// Without this the scan produces a finding a week about whoever was quietest: one void out of
    /// three sales is 33%, and it means nothing.
    pub minimum_sales: usize,
    /// How many times everyone else's rate counts as standing out.
    pub outlier_multiple: u32,
}

impl Sensitivity {
    /// A starting point, to be tuned against real shops.
    ///
    /// **Both numbers are labelled guesses.** Nobody has watched a real till with this running
    /// yet, and the right values are an empirical question about how noisy a real day is. They
    /// live here, named, rather than scattered as literals through the detectors.
    #[must_use]
    pub const fn starting_point() -> Self {
        Self {
            minimum_sales: 20,
            outlier_multiple: 2,
        }
    }
}

/// A period's activity, as the scan needs to see it.
///
/// Borrowed rather than owned: this runs over a projection that already exists, and copying a day
/// of sales to look at them would be the most expensive part of the whole feature.
#[derive(Debug, Clone, Copy)]
pub struct Activity<'a> {
    /// Completed sales only. An open ticket has not finished being what it is.
    pub sales: &'a [&'a Sale],
    pub audit: &'a [AuditEntry],
    /// Who held which role at the time of reading, not at the time of the event. Roles live in the
    /// staff directory rather than the log, so this answer can change after the fact.
    pub roles: &'a BTreeMap<Uuid, Role>,
    pub currency: Currency,
}

/// Everything the log has to say about a period.
///
/// # Errors
/// [`SaleError`] if a sale's own totals cannot be computed — which means the sale is malformed,
/// not that the scan failed.
pub fn scan(activity: &Activity<'_>, sensitivity: &Sensitivity) -> Result<Vec<Finding>, SaleError> {
    let mut findings = Vec::new();

    findings.extend(self_approvals(activity));
    findings.extend(discount_outliers(activity, sensitivity)?);
    findings.extend(void_outliers(activity, sensitivity));

    Ok(ranked(findings))
}

/// Someone approving their own action without the authority to have done it unaided.
///
/// The strongest signal in here, because it does not depend on comparing anyone to anyone: the
/// approval step exists to put a second person in the loop, and this counts the times there was
/// only one.
fn self_approvals(activity: &Activity<'_>) -> Vec<Finding> {
    let bypassed = unapproved(activity.audit, |staff_id| {
        activity.roles.get(&staff_id).copied()
    });

    let mut per_person: BTreeMap<Uuid, usize> = BTreeMap::new();
    for entry in bypassed {
        let seen = per_person.entry(entry.actor).or_default();
        *seen = seen.saturating_add(1);
    }

    per_person
        .into_iter()
        .map(|(staff_id, count)| Finding {
            kind: "self_approved",
            severity: Severity::Alert,
            subject: Subject::Person { staff_id },
            count,
            amount: None,
            summary: format!(
                "Approved their own action {count} time{} without holding that authority",
                plural(count)
            ),
        })
        .collect()
}

/// Someone discounting a much larger share of their takings than *everyone else* does.
///
/// Against everyone else rather than against the outlet, which is not a nicety. A pool that
/// contains the person being judged dampens exactly the signal being looked for, and with two
/// people on the rota it caps the possible ratio at two — so the one person doing all the
/// discounting can never exceed a threshold of two, no matter how extreme they are.
fn discount_outliers(
    activity: &Activity<'_>,
    sensitivity: &Sensitivity,
) -> Result<Vec<Finding>, SaleError> {
    let mut per_person: BTreeMap<Uuid, Tally> = BTreeMap::new();
    let mut outlet = Tally::default();

    for sale in activity.sales {
        // A sale with every line struck off has no totals to compute. It still happened, and it
        // still counts for the void rate — it simply contributes no money.
        let Ok(totals) = sale.totals() else { continue };
        let person = per_person.entry(sale.opened_by()).or_default();

        person.gross = person.gross.saturating_add(totals.gross.minor());
        person.part = person.part.saturating_add(totals.discount.minor());
        person.sales = person.sales.saturating_add(1);

        outlet.gross = outlet.gross.saturating_add(totals.gross.minor());
        outlet.part = outlet.part.saturating_add(totals.discount.minor());
        outlet.sales = outlet.sales.saturating_add(1);
    }

    let mut findings = Vec::new();
    for (person, tally) in &per_person {
        if tally.sales < sensitivity.minimum_sales || tally.part == 0 || tally.gross == 0 {
            continue;
        }
        let others = outlet.without(tally);
        if !stands_out(tally, &others, sensitivity.outlier_multiple) {
            continue;
        }

        findings.push(Finding {
            kind: "discount_rate_outlier",
            severity: Severity::Notable,
            subject: Subject::Person { staff_id: *person },
            count: tally.sales,
            amount: Some(Money::from_minor(tally.part, activity.currency)),
            summary: format!(
                "Discounted more than {}× what everyone else did, across {} sales",
                sensitivity.outlier_multiple, tally.sales
            ),
        });
    }

    Ok(findings)
}

/// Someone voiding lines far more often than everyone else.
///
/// Counted per line rather than per sale: one sale with eight voided lines is the signal, and
/// counting sales would hide it entirely.
fn void_outliers(activity: &Activity<'_>, sensitivity: &Sensitivity) -> Vec<Finding> {
    let mut per_person: BTreeMap<Uuid, Tally> = BTreeMap::new();
    let mut outlet = Tally::default();

    for sale in activity.sales {
        let voided = i64::try_from(sale.void_count()).unwrap_or(i64::MAX);
        let total = i64::try_from(sale.lines().len()).unwrap_or(i64::MAX);
        let person = per_person.entry(sale.opened_by()).or_default();

        person.gross = person.gross.saturating_add(total);
        person.part = person.part.saturating_add(voided);
        person.sales = person.sales.saturating_add(1);

        outlet.gross = outlet.gross.saturating_add(total);
        outlet.part = outlet.part.saturating_add(voided);
        outlet.sales = outlet.sales.saturating_add(1);
    }

    let mut findings = Vec::new();
    for (person, tally) in &per_person {
        if tally.sales < sensitivity.minimum_sales || tally.part == 0 || tally.gross == 0 {
            continue;
        }
        let others = outlet.without(tally);
        if !stands_out(tally, &others, sensitivity.outlier_multiple) {
            continue;
        }

        findings.push(Finding {
            kind: "void_rate_outlier",
            severity: Severity::Notable,
            subject: Subject::Person { staff_id: *person },
            count: usize::try_from(tally.part).unwrap_or_default(),
            amount: None,
            summary: format!(
                "Voided {} line{} out of {} — more than {}× what everyone else did",
                tally.part,
                plural(usize::try_from(tally.part).unwrap_or_default()),
                tally.gross,
                sensitivity.outlier_multiple
            ),
        });
    }

    findings
}

/// A share of something: `part` out of `gross`, across `sales` sales.
#[derive(Debug, Clone, Copy, Default)]
struct Tally {
    part: i64,
    gross: i64,
    sales: usize,
}

impl Tally {
    /// This tally's contribution removed, leaving everyone else's.
    fn without(&self, person: &Self) -> Self {
        Self {
            part: self.part.saturating_sub(person.part),
            gross: self.gross.saturating_sub(person.gross),
            sales: self.sales.saturating_sub(person.sales),
        }
    }
}

/// Whether `person`'s rate exceeds `multiple` times everyone else's.
///
/// Cross-multiplied rather than divided: both sides are exact integers, and a rate rounded to a
/// percentage would make a small shop's numbers disagree with themselves.
///
/// Nobody else doing it at all counts as standing out, provided this person did — being the only
/// one is the strongest form of the signal, not an absence of one.
fn stands_out(person: &Tally, others: &Tally, multiple: u32) -> bool {
    if others.gross == 0 || others.part == 0 {
        return person.part > 0;
    }

    let left = i128::from(person.part).saturating_mul(i128::from(others.gross));
    let right = i128::from(others.part)
        .saturating_mul(i128::from(person.gross))
        .saturating_mul(i128::from(multiple));

    left > right
}

const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Rounding;
    use crate::quantity::Quantity;
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
        Money::from_minor(minor, BDT)
    }

    const CASHIER: u128 = 0xCA;
    const MANAGER: u128 = 0x11A;

    fn roles() -> BTreeMap<Uuid, Role> {
        BTreeMap::from([(id(CASHIER), Role::Cashier), (id(MANAGER), Role::Manager)])
    }

    /// A completed sale of `lines` lines, `voided` of them struck off, rung by `person`.
    fn sale(base: u128, person: u128, lines: usize, voided: usize) -> Sale {
        let mut events = vec![SaleEvent::Opened {
            sale_id: id(base),
            opened_by: id(person),
            currency: BDT,
            pricing_mode: PricingMode::TaxInclusive,
            rounding: Rounding::HalfUp,
        }];
        for index in 0..lines {
            events.push(SaleEvent::LineAdded {
                sale_id: id(base),
                line_id: id(base + 100 + index as u128),
                product_id: id(9),
                name: "Item".to_owned(),
                unit_price: bdt(10_000),
                quantity: Quantity::ONE,
                tax_class: TaxClass::standard(1500),
                modifiers: Vec::new(),
            });
        }
        for index in 0..voided {
            events.push(SaleEvent::LineVoided {
                sale_id: id(base),
                line_id: id(base + 100 + index as u128),
                reason: VoidReason::Mistake,
                authorized_by: id(MANAGER),
            });
        }
        Sale::replay(&events).expect("valid")
    }

    /// The same, with an order-level discount.
    fn discounted_sale(base: u128, person: u128, discount_minor: i64) -> Sale {
        let mut sale = sale(base, person, 1, 0);
        sale.apply(&SaleEvent::OrderDiscounted {
            sale_id: id(base),
            discount: Discount::Amount {
                amount: bdt(discount_minor),
            },
            authorized_by: id(MANAGER),
        })
        .expect("discounts");
        sale
    }

    fn audit(
        kind: &'static str,
        actor: u128,
        approved_by: u128,
        amount: Option<Money>,
    ) -> AuditEntry {
        AuditEntry {
            at: at(0),
            severity: Severity::Notable,
            kind,
            actor: id(actor),
            approved_by: Some(id(approved_by)),
            amount,
            summary: String::new(),
        }
    }

    fn activity<'a>(
        sales: &'a [&'a Sale],
        entries: &'a [AuditEntry],
        role_table: &'a BTreeMap<Uuid, Role>,
    ) -> Activity<'a> {
        Activity {
            sales,
            audit: entries,
            roles: role_table,
            currency: BDT,
        }
    }

    fn sensitivity() -> Sensitivity {
        Sensitivity {
            minimum_sales: 2,
            outlier_multiple: 2,
        }
    }

    #[test]
    fn a_cashier_approving_their_own_void_is_an_alert() {
        // The approval step exists to put a second person in the loop. This counts the times
        // there was only one.
        let entries = vec![audit("sale.line_voided", CASHIER, CASHIER, None)];
        let table = roles();
        let found = scan(&activity(&[], &entries, &table), &sensitivity()).expect("scans");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "self_approved");
        assert_eq!(found[0].severity, Severity::Alert);
        assert_eq!(found[0].person(), Some(id(CASHIER)));
    }

    #[test]
    fn a_manager_approving_their_own_void_is_not_a_finding() {
        // A manager alone at seven in the morning is the ordinary case, not a signal.
        let entries = vec![audit("sale.line_voided", MANAGER, MANAGER, None)];
        let table = roles();
        let found = scan(&activity(&[], &entries, &table), &sensitivity()).expect("scans");

        assert!(found.is_empty());
    }

    #[test]
    fn a_void_a_manager_approved_for_someone_else_is_not_a_finding() {
        let entries = vec![audit("sale.line_voided", CASHIER, MANAGER, None)];
        let table = roles();
        let found = scan(&activity(&[], &entries, &table), &sensitivity()).expect("scans");

        assert!(found.is_empty());
    }

    #[test]
    fn repeated_self_approvals_are_counted_rather_than_repeated() {
        let entries = vec![
            audit("sale.line_voided", CASHIER, CASHIER, None),
            audit("sale.line_voided", CASHIER, CASHIER, None),
            audit("sale.order_discounted", CASHIER, CASHIER, Some(bdt(500))),
        ];
        let table = roles();
        let found = scan(&activity(&[], &entries, &table), &sensitivity()).expect("scans");

        assert_eq!(found.len(), 1, "one finding about one person");
        assert_eq!(found[0].count, 3);
    }

    #[test]
    fn a_cashier_voiding_far_more_than_the_outlet_stands_out() {
        let heavy_first = sale(0x100, CASHIER, 4, 3);
        let heavy_second = sale(0x200, CASHIER, 4, 3);
        let light_first = sale(0x300, MANAGER, 4, 0);
        let light_second = sale(0x400, MANAGER, 4, 0);
        let sales = [&heavy_first, &heavy_second, &light_first, &light_second];
        let table = roles();

        let found = scan(&activity(&sales, &[], &table), &sensitivity()).expect("scans");
        let void_finding = found
            .iter()
            .find(|finding| finding.kind == "void_rate_outlier")
            .expect("found");

        assert_eq!(void_finding.person(), Some(id(CASHIER)));
        assert_eq!(void_finding.count, 6, "lines, not sales");
    }

    #[test]
    fn nobody_stands_out_when_everybody_voids_the_same_amount() {
        let first = sale(0x100, CASHIER, 4, 1);
        let second = sale(0x200, CASHIER, 4, 1);
        let third = sale(0x300, MANAGER, 4, 1);
        let fourth = sale(0x400, MANAGER, 4, 1);
        let sales = [&first, &second, &third, &fourth];
        let table = roles();

        let found = scan(&activity(&sales, &[], &table), &sensitivity()).expect("scans");
        assert!(
            found
                .iter()
                .all(|finding| finding.kind != "void_rate_outlier")
        );
    }

    #[test]
    fn somebody_too_quiet_to_compare_is_left_alone() {
        // One void out of three sales is 33% and it means nothing. Without this the scan produces
        // a finding a week about whoever was quietest.
        let busy_first = sale(0x100, MANAGER, 4, 0);
        let busy_second = sale(0x200, MANAGER, 4, 0);
        let busy_third = sale(0x300, MANAGER, 4, 0);
        let lone = sale(0x400, CASHIER, 1, 1);
        let sales = [&busy_first, &busy_second, &busy_third, &lone];
        let table = roles();

        let found = scan(&activity(&sales, &[], &table), &sensitivity()).expect("scans");
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_cashier_discounting_far_more_than_the_outlet_stands_out() {
        let heavy_first = discounted_sale(0x100, CASHIER, 4_000);
        let heavy_second = discounted_sale(0x200, CASHIER, 4_000);
        let light_first = discounted_sale(0x300, MANAGER, 100);
        let light_second = discounted_sale(0x400, MANAGER, 100);
        let sales = [&heavy_first, &heavy_second, &light_first, &light_second];
        let table = roles();

        let found = scan(&activity(&sales, &[], &table), &sensitivity()).expect("scans");
        let discount_finding = found
            .iter()
            .find(|finding| finding.kind == "discount_rate_outlier")
            .expect("found");

        assert_eq!(discount_finding.person(), Some(id(CASHIER)));
        assert_eq!(discount_finding.amount, Some(bdt(8_000)));
    }

    #[test]
    fn a_shop_that_discounts_nothing_produces_no_discount_findings() {
        let first = sale(0x100, CASHIER, 2, 0);
        let second = sale(0x200, CASHIER, 2, 0);
        let sales = [&first, &second];
        let table = roles();

        let found = scan(&activity(&sales, &[], &table), &sensitivity()).expect("scans");
        assert!(found.is_empty());
    }

    #[test]
    fn the_only_person_doing_something_stands_out_even_against_two_people() {
        // Comparing someone to a pool that contains them caps the possible ratio at the number of
        // people on the rota. With two, the one doing all the voiding can never exceed 2× — so the
        // comparison is against everyone *else*, and this is the case that proves it.
        let heavy_first = sale(0x100, CASHIER, 4, 4);
        let heavy_second = sale(0x200, CASHIER, 4, 4);
        let clean_first = sale(0x300, MANAGER, 4, 0);
        let clean_second = sale(0x400, MANAGER, 4, 0);
        let sales = [&heavy_first, &heavy_second, &clean_first, &clean_second];
        let table = roles();

        let found = scan(&activity(&sales, &[], &table), &sensitivity()).expect("scans");
        assert!(
            found
                .iter()
                .any(|finding| finding.kind == "void_rate_outlier"),
            "{found:?}"
        );
    }

    #[test]
    fn a_sale_with_every_line_voided_still_counts_towards_voids() {
        // It has no totals to compute — and it is precisely the sale worth noticing, so failing
        // the whole scan on it would blind the feature to its best signal.
        let emptied_first = sale(0x100, CASHIER, 2, 2);
        let emptied_second = sale(0x200, CASHIER, 2, 2);
        let ordinary_first = sale(0x300, MANAGER, 2, 0);
        let ordinary_second = sale(0x400, MANAGER, 2, 0);
        let sales = [
            &emptied_first,
            &emptied_second,
            &ordinary_first,
            &ordinary_second,
        ];
        let table = roles();

        let found = scan(&activity(&sales, &[], &table), &sensitivity()).expect("scans");
        let voids = found
            .iter()
            .find(|finding| finding.kind == "void_rate_outlier")
            .expect("found");

        assert_eq!(voids.count, 4);
    }

    #[test]
    fn an_empty_day_says_nothing() {
        let table = roles();
        assert!(
            scan(&activity(&[], &[], &table), &sensitivity())
                .expect("scans")
                .is_empty()
        );
    }

    #[test]
    fn the_alert_comes_before_the_merely_notable() {
        let heavy_first = sale(0x100, CASHIER, 4, 3);
        let heavy_second = sale(0x200, CASHIER, 4, 3);
        let light_first = sale(0x300, MANAGER, 4, 0);
        let light_second = sale(0x400, MANAGER, 4, 0);
        let sales = [&heavy_first, &heavy_second, &light_first, &light_second];
        let entries = vec![audit("sale.line_voided", CASHIER, CASHIER, None)];
        let table = roles();

        let found = scan(&activity(&sales, &entries, &table), &sensitivity()).expect("scans");
        assert_eq!(found[0].kind, "self_approved");
    }

    #[test]
    fn an_unknown_actor_approving_themselves_is_still_flagged() {
        // A deleted or unknown id approving its own void is more alarming than a known one.
        let entries = vec![audit("sale.line_voided", 0xDEAD, 0xDEAD, None)];
        let table = roles();
        let found = scan(&activity(&[], &entries, &table), &sensitivity()).expect("scans");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].person(), Some(id(0xDEAD)));
    }

    #[test]
    fn the_starting_point_asks_for_enough_sales_to_mean_something() {
        let sensitivity = Sensitivity::starting_point();
        assert!(
            sensitivity.minimum_sales > 1,
            "one sale compares to nothing"
        );
        assert!(sensitivity.outlier_multiple > 1);
    }
}
