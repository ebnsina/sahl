use core::fmt;

use serde::{Deserialize, Serialize};

use super::error::MoneyError;

/// The currencies Sahl transacts in.
///
/// Deliberately a closed enum rather than an open string. An unknown currency reaching the VAT
/// engine is not something to handle gracefully at the till — it is a configuration error that
/// should have failed at startup.
///
/// All four current members happen to use two minor digits, but [`Currency::exponent`] is written
/// as an exhaustive match so that adding KWD (three digits) or JPY (zero) is a one-line change that
/// the compiler forces you to consider everywhere it matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[non_exhaustive]
pub enum Currency {
    /// Bangladeshi taka.
    Bdt,
    /// Saudi riyal.
    Sar,
    /// UAE dirham.
    Aed,
    /// US dollar — used for SaaS billing, not for merchant tills.
    Usd,
}

impl Currency {
    /// ISO-4217 alphabetic code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Bdt => "BDT",
            Self::Sar => "SAR",
            Self::Aed => "AED",
            Self::Usd => "USD",
        }
    }

    /// Number of decimal digits in the minor unit (ISO-4217 exponent).
    #[must_use]
    pub const fn exponent(self) -> u8 {
        match self {
            Self::Bdt | Self::Sar | Self::Aed | Self::Usd => 2,
        }
    }

    /// Minor units per major unit — 100 for a two-digit currency.
    ///
    /// Written as a match rather than `10i64.pow(exponent)` so it stays a `const fn` and cannot
    /// panic or overflow.
    #[must_use]
    pub const fn minor_per_major(self) -> i64 {
        match self.exponent() {
            0 => 1,
            1 => 10,
            2 => 100,
            3 => 1_000,
            // Unreachable for the current set; returning the safest value rather than panicking
            // keeps this const and total.
            _ => 100,
        }
    }

    /// Parse an ISO-4217 code, case-insensitively.
    ///
    /// # Errors
    /// Returns [`MoneyError::UnknownCurrency`] for any code outside the supported set.
    pub fn from_code(code: &str) -> Result<Self, MoneyError> {
        match code.to_ascii_uppercase().as_str() {
            "BDT" => Ok(Self::Bdt),
            "SAR" => Ok(Self::Sar),
            "AED" => Ok(Self::Aed),
            "USD" => Ok(Self::Usd),
            other => Err(MoneyError::UnknownCurrency(other.to_owned())),
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_its_code() {
        for currency in [Currency::Bdt, Currency::Sar, Currency::Aed, Currency::Usd] {
            assert_eq!(Currency::from_code(currency.code()), Ok(currency));
        }
    }

    #[test]
    fn parses_case_insensitively() {
        assert_eq!(Currency::from_code("bdt"), Ok(Currency::Bdt));
        assert_eq!(Currency::from_code("sAr"), Ok(Currency::Sar));
    }

    #[test]
    fn rejects_unknown_codes_rather_than_guessing() {
        assert_eq!(
            Currency::from_code("XYZ"),
            Err(MoneyError::UnknownCurrency("XYZ".to_owned()))
        );
    }

    #[test]
    fn minor_per_major_matches_exponent() {
        for currency in [Currency::Bdt, Currency::Sar, Currency::Aed, Currency::Usd] {
            assert_eq!(currency.exponent(), 2);
            assert_eq!(currency.minor_per_major(), 100);
        }
    }
}
