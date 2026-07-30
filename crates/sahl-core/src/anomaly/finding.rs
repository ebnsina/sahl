//! One thing worth an owner's attention.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::Money;
use crate::staff::Severity;

/// Who or what a finding concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Subject {
    /// One person, by staff id.
    Person { staff_id: Uuid },
    /// The outlet as a whole — nobody in particular is being pointed at.
    Outlet,
}

/// Something the log says, phrased for someone who has to decide what it means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable machine-readable kind, for filtering and for suppressing one an owner has dismissed.
    pub kind: &'static str,
    pub severity: Severity,
    pub subject: Subject,
    /// How many times it happened. The count is the evidence; a single occurrence of most of these
    /// is a Tuesday.
    pub count: usize,
    /// The money involved, where the signal is about money rather than frequency.
    pub amount: Option<Money>,
    /// One line stating what was counted — never what it implies.
    pub summary: String,
}

impl Finding {
    /// The person this concerns, if it concerns one.
    #[must_use]
    pub const fn person(&self) -> Option<Uuid> {
        match self.subject {
            Subject::Person { staff_id } => Some(staff_id),
            Subject::Outlet => None,
        }
    }
}

/// Order a feed for reading: most severe first, then most frequent.
///
/// Same reasoning as the audit feed — a list sorted by count alone buries one alarming thing under
/// a hundred ordinary ones.
#[must_use]
pub fn ranked(mut findings: Vec<Finding>) -> Vec<Finding> {
    findings.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then(right.count.cmp(&left.count))
            .then(left.kind.cmp(right.kind))
    });
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(kind: &'static str, severity: Severity, count: usize) -> Finding {
        Finding {
            kind,
            severity,
            subject: Subject::Outlet,
            count,
            amount: None,
            summary: String::new(),
        }
    }

    #[test]
    fn the_most_severe_finding_comes_first_regardless_of_count() {
        let ranked = ranked(vec![
            finding("routine", Severity::Routine, 100),
            finding("alert", Severity::Alert, 1),
        ]);

        assert_eq!(ranked[0].kind, "alert");
    }

    #[test]
    fn equally_severe_findings_are_ordered_by_how_often_they_happened() {
        let ranked = ranked(vec![
            finding("rare", Severity::Notable, 2),
            finding("common", Severity::Notable, 9),
        ]);

        assert_eq!(ranked[0].kind, "common");
    }

    #[test]
    fn a_finding_about_the_outlet_names_nobody() {
        assert_eq!(finding("x", Severity::Notable, 1).person(), None);
    }
}
