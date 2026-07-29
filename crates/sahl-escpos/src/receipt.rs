//! Receipt layout — column arithmetic over a fixed-width character grid.
//!
//! Width is a type, not a number: a wrong column count wraps every price onto its own line.

use sahl_core::Money;

/// Paper width, in printable character columns at the default font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperWidth {
    /// 58mm — the common cheap roll, and what most handheld printers take.
    Mm58,
    /// 80mm — counter-top printers.
    Mm80,
}

impl PaperWidth {
    #[must_use]
    pub const fn columns(self) -> usize {
        match self {
            Self::Mm58 => 32,
            Self::Mm80 => 48,
        }
    }

    /// Printable dots across, for raster images.
    #[must_use]
    pub const fn dots(self) -> u32 {
        match self {
            // Both are 203dpi; the printable area is slightly under the paper width.
            Self::Mm58 => 384,
            Self::Mm80 => 576,
        }
    }
}

/// Label left, amount hard right. Truncates the label rather than wrapping the price.
#[must_use]
pub fn columns(label: &str, amount: &str, width: usize) -> String {
    let amount_width = amount.chars().count();
    if amount_width >= width {
        return amount.chars().take(width).collect();
    }

    let room = width.saturating_sub(amount_width).saturating_sub(1);
    let label_width = label.chars().count();

    if label_width > room {
        let truncated: String = label.chars().take(room.saturating_sub(1)).collect();
        format!("{truncated}… {amount}")
    } else {
        let padding = width
            .saturating_sub(label_width)
            .saturating_sub(amount_width);
        format!("{label}{}{amount}", " ".repeat(padding))
    }
}

/// Centre a line within the paper width.
#[must_use]
pub fn centered(text: &str, width: usize) -> String {
    let length = text.chars().count();
    if length >= width {
        return text.chars().take(width).collect();
    }
    let left = width.saturating_sub(length) / 2;
    format!("{}{text}", " ".repeat(left))
}

/// A full-width rule.
#[must_use]
pub fn rule(width: usize, character: char) -> String {
    character.to_string().repeat(width)
}

/// Exact amount as a plain decimal string.
///
/// Not locale-formatted: grouping varies a number's width and breaks column alignment.
#[must_use]
pub fn amount(money: Money) -> String {
    let exponent = usize::from(money.currency().exponent());
    let minor = money.minor();
    let negative = minor < 0;
    let digits = minor.unsigned_abs().to_string();
    let padding = exponent.saturating_add(1).saturating_sub(digits.len());
    let padded = format!("{}{digits}", "0".repeat(padding));

    let split = padded.len().saturating_sub(exponent);
    let whole = padded.get(..split).unwrap_or("0");
    let fraction = padded.get(split..).unwrap_or("");
    let sign = if negative { "-" } else { "" };

    if exponent == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sahl_core::Currency;

    fn bdt(minor: i64) -> Money {
        Money::from_minor(minor, Currency::Bdt)
    }

    #[test]
    fn paper_widths_match_their_column_counts() {
        assert_eq!(PaperWidth::Mm58.columns(), 32);
        assert_eq!(PaperWidth::Mm80.columns(), 48);
    }

    #[test]
    fn columns_pad_the_amount_to_the_right_edge() {
        let line = columns("Rice 5kg", "480.00", 32);
        assert_eq!(line.chars().count(), 32);
        assert!(line.starts_with("Rice 5kg"));
        assert!(line.ends_with("480.00"));
    }

    #[test]
    fn a_long_label_is_truncated_rather_than_wrapping_the_price() {
        // A price that wraps is a price a customer cannot find.
        let line = columns(
            "Extremely long imported product name that will not fit",
            "1234.00",
            32,
        );
        assert_eq!(line.chars().count(), 32);
        assert!(line.ends_with("1234.00"));
        assert!(line.contains('…'));
    }

    #[test]
    fn an_amount_wider_than_the_paper_is_clipped_not_wrapped() {
        let line = columns("x", "123456789012345678901234567890123456", 32);
        assert_eq!(line.chars().count(), 32);
    }

    #[test]
    fn centering_is_stable_at_the_edges() {
        assert_eq!(centered("ab", 6), "  ab");
        assert_eq!(centered("abcdef", 6), "abcdef");
        assert_eq!(centered("abcdefgh", 6).chars().count(), 6);
    }

    #[test]
    fn amounts_render_with_a_fixed_decimal_width() {
        // Fixed width is the point: locale grouping would vary the width and break alignment.
        assert_eq!(amount(bdt(48_000)), "480.00");
        assert_eq!(amount(bdt(5)), "0.05");
        assert_eq!(amount(bdt(0)), "0.00");
        assert_eq!(amount(bdt(-4_500)), "-45.00");
        assert_eq!(amount(bdt(100)), "1.00");
    }

    #[test]
    fn a_sub_unit_amount_keeps_its_leading_zero() {
        assert_eq!(amount(bdt(7)), "0.07");
        assert_eq!(amount(bdt(-7)), "-0.07");
    }
}
