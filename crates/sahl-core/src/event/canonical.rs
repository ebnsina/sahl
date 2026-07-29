//! Canonical serialization — the exact bytes an event is hashed over.
//!
//! Two processes must derive the same digest from the same event, or the chain breaks. That is
//! harder than it sounds: normal JSON serialization is free to vary key order, whitespace, and
//! number formatting between implementations and versions.
//!
//! The rules here are deliberately narrow:
//!
//! 1. **Keys are sorted.** Achieved by routing through `serde_json::Value`, whose object type is a
//!    `BTreeMap` (the workspace deliberately does not enable `preserve_order`). Serializing a
//!    struct directly would emit fields in declaration order, so reordering a struct's fields would
//!    silently invalidate every hash ever computed.
//! 2. **No whitespace.** `to_vec` is compact by default.
//! 3. **No floating-point numbers, ever.** Rejected at runtime rather than trusted, because a float
//!    has no single canonical decimal form. This is the same rule the money types enforce at
//!    compile time, extended to anything that reaches the chain.

use serde::Serialize;
use serde_json::Value;

use super::error::EventError;

/// Serialize `value` to the canonical byte form used for hashing.
///
/// # Errors
/// [`EventError::NotCanonical`] if the value cannot be represented, or contains a float.
pub fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, EventError> {
    let json = serde_json::to_value(value).map_err(|source| EventError::NotCanonical {
        reason: source.to_string(),
    })?;

    reject_floats(&json)?;

    serde_json::to_vec(&json).map_err(|source| EventError::NotCanonical {
        reason: source.to_string(),
    })
}

/// Walk a value and refuse any floating-point number.
///
/// A float reaching the event log is always a bug — money is integer minor units and quantity is
/// integer thousandths — but if one ever did, it would produce a digest that varies by platform.
/// Failing loudly here beats a chain that mysteriously fails to verify on one device.
fn reject_floats(value: &Value) -> Result<(), EventError> {
    match value {
        Value::Number(number) if number.as_f64().is_some() && !is_integral(number) => {
            Err(EventError::NotCanonical {
                reason: format!("floating-point number {number} cannot be canonically hashed"),
            })
        }
        Value::Array(items) => items.iter().try_for_each(reject_floats),
        Value::Object(entries) => entries.values().try_for_each(reject_floats),
        _ => Ok(()),
    }
}

fn is_integral(number: &serde_json::Number) -> bool {
    number.is_i64() || number.is_u64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Declared {
        zebra: u32,
        alpha: u32,
        middle: u32,
    }

    #[derive(Serialize)]
    struct Reordered {
        alpha: u32,
        middle: u32,
        zebra: u32,
    }

    #[test]
    fn keys_are_sorted_regardless_of_declaration_order() {
        // This is the property that lets struct fields be reordered without invalidating every
        // hash ever written to a merchant's device.
        let declared = canonical_bytes(&Declared {
            zebra: 3,
            alpha: 1,
            middle: 2,
        })
        .expect("canonicalises");
        let reordered = canonical_bytes(&Reordered {
            alpha: 1,
            middle: 2,
            zebra: 3,
        })
        .expect("canonicalises");

        assert_eq!(declared, reordered);
        assert_eq!(
            String::from_utf8(declared).expect("utf-8"),
            r#"{"alpha":1,"middle":2,"zebra":3}"#
        );
    }

    #[test]
    fn output_is_compact() {
        let bytes = canonical_bytes(&serde_json::json!({"a": [1, 2]})).expect("canonicalises");
        let text = String::from_utf8(bytes).expect("utf-8");
        assert!(
            !text.contains(' '),
            "canonical output must not contain whitespace"
        );
    }

    #[test]
    fn nested_objects_are_sorted_too() {
        let bytes = canonical_bytes(&serde_json::json!({
            "outer": { "z": 1, "a": 2 },
            "another": 3,
        }))
        .expect("canonicalises");

        assert_eq!(
            String::from_utf8(bytes).expect("utf-8"),
            r#"{"another":3,"outer":{"a":2,"z":1}}"#
        );
    }

    #[test]
    fn array_order_is_preserved_because_it_is_meaningful() {
        let bytes = canonical_bytes(&serde_json::json!([3, 1, 2])).expect("canonicalises");
        assert_eq!(String::from_utf8(bytes).expect("utf-8"), "[3,1,2]");
    }

    #[test]
    fn floats_are_refused_rather_than_hashed() {
        let result = canonical_bytes(&serde_json::json!({ "amount": 12.34 }));
        assert!(matches!(result, Err(EventError::NotCanonical { .. })));
    }

    #[test]
    fn floats_are_refused_when_buried_in_nested_structures() {
        let result = canonical_bytes(&serde_json::json!({
            "lines": [ { "price": 1 }, { "price": 0.5 } ]
        }));
        assert!(matches!(result, Err(EventError::NotCanonical { .. })));
    }

    #[test]
    fn integers_are_accepted_including_negative_and_large() {
        assert!(canonical_bytes(&serde_json::json!({ "a": -1, "b": i64::MAX })).is_ok());
    }

    #[test]
    fn the_same_input_always_produces_the_same_bytes() {
        let value = serde_json::json!({ "kind": "sale", "total": 10_000, "lines": [1, 2, 3] });
        assert_eq!(
            canonical_bytes(&value).expect("canonicalises"),
            canonical_bytes(&value).expect("canonicalises")
        );
    }
}
