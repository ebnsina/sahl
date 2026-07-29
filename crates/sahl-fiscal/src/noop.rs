//! No fiscal regime.
//!
//! Not a stub — a real deployment. A shop below the VAT registration threshold, or one in a market
//! Sahl has not been localised for, owes its customer a receipt and owes the state nothing extra.
//! Making that an explicit regime rather than an `Option<Box<dyn Fiscalization>>` means every call
//! site handles it the same way as any other country.

use crate::{Document, FiscalError, Fiscalization, Invoice};

#[derive(Debug, Clone, Copy, Default)]
pub struct NoFiscalRegime;

impl Fiscalization for NoFiscalRegime {
    fn regime(&self) -> &'static str {
        "none"
    }

    fn issue(&self, invoice: &Invoice) -> Result<Document, FiscalError> {
        // Still refuses an empty sale. A regime that accepts nonsense is a poor stand-in for one
        // that does not, and this is what most tests run against.
        if invoice.totals.lines.is_empty() {
            return Err(FiscalError::Empty);
        }
        Ok(Document::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::invoice;

    #[test]
    fn an_ordinary_sale_needs_no_document() {
        assert_eq!(NoFiscalRegime.issue(&invoice(1)), Ok(Document::None));
        assert_eq!(NoFiscalRegime.regime(), "none");
    }

    #[test]
    fn an_empty_sale_is_still_refused() {
        let mut sale = invoice(1);
        sale.totals.lines.clear();
        assert_eq!(NoFiscalRegime.issue(&sale), Err(FiscalError::Empty));
    }
}
