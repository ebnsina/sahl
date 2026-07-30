//! Whether a weight may become a sale line.
//!
//! Separate from parsing on purpose. A label parses fine and still describes something nobody
//! should be charged for — a jammed scale prints 0.000 kg all day, and it scans cleanly.

use crate::catalogue::Unit;
use crate::quantity::Quantity;

/// Check a weight against the unit it is being sold in.
///
/// # Errors
/// [`WeighError`] for a zero, negative, or fractional-where-it-cannot-be quantity.
pub fn weigh(unit: Unit, quantity: Quantity) -> Result<Quantity, WeighError> {
    if quantity.is_negative() {
        return Err(WeighError::Negative { quantity });
    }
    if quantity.is_zero() {
        return Err(WeighError::Nothing);
    }
    // Selling 0.4 of a piece is a mis-key; selling 0.4 kg is Tuesday.
    if !unit.is_divisible() && quantity.milli() % Quantity::MILLI_PER_UNIT != 0 {
        return Err(WeighError::NotDivisible { unit, quantity });
    }
    Ok(quantity)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WeighError {
    #[error("the scale read nothing — check the item is on the pan")]
    Nothing,

    #[error("a weight cannot be negative, got {quantity} — tare the scale")]
    Negative { quantity: Quantity },

    #[error("{} is sold whole, so {quantity} is a mis-key", .unit.label())]
    NotDivisible { unit: Unit, quantity: Quantity },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_weight_passes_through_unchanged() {
        let weight = Quantity::from_milli(1_250);
        assert_eq!(weigh(Unit::Kilogram, weight), Ok(weight));
    }

    #[test]
    fn a_jammed_scale_reading_zero_is_refused() {
        // It scans cleanly and parses cleanly. This is the only place it gets caught.
        assert_eq!(
            weigh(Unit::Kilogram, Quantity::ZERO),
            Err(WeighError::Nothing)
        );
    }

    #[test]
    fn a_negative_reading_is_refused_rather_than_credited() {
        let quantity = Quantity::from_milli(-500);
        assert_eq!(
            weigh(Unit::Kilogram, quantity),
            Err(WeighError::Negative { quantity })
        );
    }

    #[test]
    fn a_fraction_of_a_piece_is_a_mis_key() {
        let quantity = Quantity::from_milli(400);
        assert_eq!(
            weigh(Unit::Piece, quantity),
            Err(WeighError::NotDivisible {
                unit: Unit::Piece,
                quantity
            })
        );
    }

    #[test]
    fn whole_pieces_are_fine() {
        let quantity = Quantity::from_milli(3_000);
        assert_eq!(weigh(Unit::Piece, quantity), Ok(quantity));
    }

    #[test]
    fn every_divisible_unit_accepts_a_fraction() {
        for unit in Unit::all() {
            let result = weigh(unit, Quantity::from_milli(400));
            assert_eq!(result.is_ok(), unit.is_divisible(), "{unit:?}");
        }
    }
}
