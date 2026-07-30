//! What a product may be ordered with.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::Money;

use super::product::CatalogueError;

/// One choice within a group — "Large", "Oat milk", "No sugar".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModifierOption {
    pub id: Uuid,
    pub name: String,
    /// What choosing it adds to **one unit**. Zero is ordinary — "no sugar" costs nothing — and
    /// negative is real: "no cheese, less 20".
    pub price_delta: Money,
}

/// A set of choices offered on a product.
///
/// Grouped rather than a flat list because the two shapes behave differently and conflating them
/// produces nonsense orders. "Size" is exactly one of small, medium, large; "extras" is any number
/// of them. A flat list lets a cashier pick small *and* large, and nothing downstream can tell that
/// is wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModifierGroup {
    pub id: Uuid,
    /// "Size", "Milk", "Extras".
    pub name: String,
    /// Fewest choices that must be made. One means the cashier cannot skip it.
    pub min: u8,
    /// Most that may be made. One makes it a single choice.
    pub max: u8,
    pub options: Vec<ModifierOption>,
}

impl ModifierGroup {
    /// Whether the cashier must choose before the line can be rung.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        self.min > 0
    }

    /// Whether exactly one choice is allowed — a radio rather than a checkbox.
    #[must_use]
    pub const fn is_single_choice(&self) -> bool {
        self.max == 1
    }

    /// # Errors
    /// [`CatalogueError`] naming what is wrong.
    pub fn validate(&self) -> Result<(), CatalogueError> {
        if self.name.trim().is_empty() {
            return Err(CatalogueError::Blank {
                field: "group name",
            });
        }
        if self.options.is_empty() {
            return Err(CatalogueError::Blank {
                field: "group options",
            });
        }
        if self
            .options
            .iter()
            .any(|option| option.name.trim().is_empty())
        {
            return Err(CatalogueError::Blank {
                field: "option name",
            });
        }
        if self.max == 0 || self.min > self.max {
            // A group nobody can satisfy would block every sale of the product it is attached to.
            return Err(CatalogueError::BadGroupBounds {
                name: self.name.clone(),
                min: self.min,
                max: self.max,
            });
        }
        if usize::from(self.min) > self.options.len() {
            return Err(CatalogueError::BadGroupBounds {
                name: self.name.clone(),
                min: self.min,
                max: self.max,
            });
        }
        Ok(())
    }

    /// Check a set of chosen option ids against this group.
    ///
    /// # Errors
    /// [`CatalogueError::ChoiceCount`] if too few or too many were chosen.
    pub fn check(&self, chosen: &[Uuid]) -> Result<(), CatalogueError> {
        let count = chosen
            .iter()
            .filter(|id| self.options.iter().any(|option| option.id == **id))
            .count();

        if count < usize::from(self.min) || count > usize::from(self.max) {
            return Err(CatalogueError::ChoiceCount {
                group: self.name.clone(),
                min: self.min,
                max: self.max,
                chosen: count,
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn option(&self, option_id: Uuid) -> Option<&ModifierOption> {
        self.options.iter().find(|option| option.id == option_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn option(n: u128, name: &str, minor: i64) -> ModifierOption {
        ModifierOption {
            id: id(n),
            name: name.to_owned(),
            price_delta: Money::from_minor(minor, Currency::Bdt),
        }
    }

    /// "Size" — exactly one, and it cannot be skipped.
    fn size() -> ModifierGroup {
        ModifierGroup {
            id: id(100),
            name: "Size".to_owned(),
            min: 1,
            max: 1,
            options: vec![
                option(1, "Small", 0),
                option(2, "Medium", 3_000),
                option(3, "Large", 6_000),
            ],
        }
    }

    /// "Extras" — any number, including none.
    fn extras() -> ModifierGroup {
        ModifierGroup {
            id: id(200),
            name: "Extras".to_owned(),
            min: 0,
            max: 3,
            options: vec![
                option(4, "Extra shot", 5_000),
                option(5, "Oat milk", 3_000),
                option(6, "No sugar", 0),
            ],
        }
    }

    #[test]
    fn a_size_group_is_required_and_single_choice() {
        assert!(size().is_required());
        assert!(size().is_single_choice());
    }

    #[test]
    fn an_extras_group_is_neither() {
        assert!(!extras().is_required());
        assert!(!extras().is_single_choice());
    }

    #[test]
    fn a_required_single_choice_refuses_none_and_refuses_two() {
        // The whole reason groups exist: a flat list lets a cashier pick small *and* large, and
        // nothing downstream can tell that is wrong.
        assert!(matches!(
            size().check(&[]),
            Err(CatalogueError::ChoiceCount { .. })
        ));
        assert_eq!(size().check(&[id(2)]), Ok(()));
        assert!(matches!(
            size().check(&[id(1), id(3)]),
            Err(CatalogueError::ChoiceCount { .. })
        ));
    }

    #[test]
    fn an_optional_group_accepts_none() {
        assert_eq!(extras().check(&[]), Ok(()));
        assert_eq!(extras().check(&[id(4), id(5)]), Ok(()));
    }

    #[test]
    fn choosing_more_than_the_maximum_is_refused() {
        let mut narrow = extras();
        narrow.max = 1;
        assert!(matches!(
            narrow.check(&[id(4), id(5)]),
            Err(CatalogueError::ChoiceCount { .. })
        ));
    }

    #[test]
    fn ids_from_another_group_do_not_count_toward_this_one() {
        // A line carries every choice across every group, so each group must count only its own —
        // otherwise picking two extras would satisfy the size requirement.
        assert!(matches!(
            size().check(&[id(4), id(5)]),
            Err(CatalogueError::ChoiceCount { .. })
        ));
    }

    #[test]
    fn a_group_with_no_options_is_refused() {
        let empty = ModifierGroup {
            options: Vec::new(),
            ..size()
        };
        assert!(matches!(
            empty.validate(),
            Err(CatalogueError::Blank { .. })
        ));
    }

    #[test]
    fn a_group_nobody_could_satisfy_is_refused() {
        // It would block every sale of the product it is attached to.
        let impossible = ModifierGroup {
            min: 4,
            max: 4,
            ..size()
        };
        assert!(matches!(
            impossible.validate(),
            Err(CatalogueError::BadGroupBounds { .. })
        ));

        let backwards = ModifierGroup {
            min: 2,
            max: 1,
            ..size()
        };
        assert!(matches!(
            backwards.validate(),
            Err(CatalogueError::BadGroupBounds { .. })
        ));
    }

    #[test]
    fn a_valid_group_is_accepted() {
        assert_eq!(size().validate(), Ok(()));
        assert_eq!(extras().validate(), Ok(()));
    }
}
