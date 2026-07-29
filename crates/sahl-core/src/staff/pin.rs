//! Till PINs.
//!
//! A PIN is four to eight digits, which is a few thousand guesses — the KDF cost is the only thing
//! standing between a stolen database and every cashier's identity. So the parameters are set here
//! rather than taken from defaults, and the rules about which PINs may be chosen are enforced here
//! rather than in a form, because a form is not the only way one gets set.
//!
//! Hashing takes its salt as an argument. That keeps this module free of randomness, so `sahl-core`
//! stays pure and the terminal and server produce identical results from identical inputs.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, Salt, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use thiserror::Error;

/// The shortest PIN we accept. Four digits is 10,000 possibilities.
pub const MIN_LENGTH: usize = 4;
/// The longest. Beyond this it stops being something a cashier types quickly.
pub const MAX_LENGTH: usize = 8;

/// 19 MiB, 2 passes, 1 lane — the OWASP-recommended second option, chosen because the terminal also
/// runs this and cheap Android hardware has to finish it while a queue waits.
const MEMORY_KIB: u32 = 19 * 1024;
const ITERATIONS: u32 = 2;
const LANES: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PinError {
    #[error("a PIN must be {MIN_LENGTH} to {MAX_LENGTH} digits, got {length}")]
    Length { length: usize },

    #[error("a PIN must be digits only")]
    NotNumeric,

    #[error("that PIN is too easy to guess")]
    Guessable,

    #[error("the stored PIN hash is not readable")]
    CorruptHash,

    #[error("hashing failed")]
    HashFailed,
}

/// Check a PIN against the rules before it is ever hashed.
///
/// # Errors
/// [`PinError`] describing why it was refused.
pub fn validate(pin: &str) -> Result<(), PinError> {
    let length = pin.chars().count();
    if !(MIN_LENGTH..=MAX_LENGTH).contains(&length) {
        return Err(PinError::Length { length });
    }
    if !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(PinError::NotNumeric);
    }
    if is_guessable(pin) {
        return Err(PinError::Guessable);
    }
    Ok(())
}

/// Hash a PIN for storage, returning a PHC string.
///
/// The salt is supplied by the caller — see the module note on why.
///
/// # Errors
/// [`PinError`] if the PIN is refused or hashing fails.
pub fn hash(pin: &str, salt: &SaltString) -> Result<String, PinError> {
    validate(pin)?;
    let salt: Salt<'_> = salt.as_salt();
    Ok(hasher()?
        .hash_password(pin.as_bytes(), salt)
        .map_err(|_| PinError::HashFailed)?
        .to_string())
}

/// Whether `pin` matches `stored`.
///
/// Deliberately does not call [`validate`] first: rules tighten over time, and a PIN that was legal
/// when it was set must keep working until it is changed. Refusing it here would lock a cashier out
/// of a till mid-shift over a policy that changed in an update.
///
/// # Errors
/// [`PinError::CorruptHash`] if `stored` is not a readable PHC string. A wrong PIN is `Ok(false)`,
/// not an error — the two must not be distinguishable to a caller by anything but the boolean.
pub fn verify(pin: &str, stored: &str) -> Result<bool, PinError> {
    let parsed = PasswordHash::new(stored).map_err(|_| PinError::CorruptHash)?;
    // Params come from the stored hash, not from `hasher()`, so PINs set under older costs keep
    // verifying after the cost is raised.
    Ok(Argon2::default()
        .verify_password(pin.as_bytes(), &parsed)
        .is_ok())
}

/// Whether a stored hash was made with parameters weaker than the current ones.
///
/// Cost has to rise as hardware does, and the only moment a PIN can be rehashed is when it is next
/// typed correctly. This is how a login knows to do that.
#[must_use]
pub fn needs_rehash(stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return true;
    };
    let Ok(params) = Params::try_from(&parsed) else {
        return true;
    };
    params.m_cost() < MEMORY_KIB || params.t_cost() < ITERATIONS
}

fn hasher() -> Result<Argon2<'static>, PinError> {
    let params =
        Params::new(MEMORY_KIB, ITERATIONS, LANES, None).map_err(|_| PinError::HashFailed)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// The PINs people actually pick.
///
/// Not a dictionary — a rule set. Repeats and runs cover the overwhelming majority of real weak
/// choices, and a list would go stale while the shapes do not.
fn is_guessable(pin: &str) -> bool {
    let digits: Vec<u8> = pin.bytes().collect();
    let Some((first, rest)) = digits.split_first() else {
        return true;
    };

    // 0000, 1111.
    if rest.iter().all(|digit| digit == first) {
        return true;
    }

    // 1234, 9876. Wrapping runs (9012) are not treated as runs; nobody picks those.
    let ascending = rest
        .iter()
        .zip(digits.iter())
        .all(|(next, previous)| next.checked_sub(*previous) == Some(1));
    let descending = rest
        .iter()
        .zip(digits.iter())
        .all(|(next, previous)| previous.checked_sub(*next) == Some(1));

    ascending || descending
}

#[cfg(test)]
mod tests {
    use super::*;

    // The tests hash for real, and the cost is deliberately high. Keep the count small.
    fn salt() -> SaltString {
        SaltString::from_b64("c2FobHRlc3RzYWx0MTIz").expect("valid b64 salt")
    }

    #[test]
    fn a_correct_pin_verifies_and_a_wrong_one_does_not() {
        let stored = hash("8317", &salt()).expect("hashes");

        assert_eq!(verify("8317", &stored), Ok(true));
        assert_eq!(verify("8318", &stored), Ok(false));
    }

    #[test]
    fn a_wrong_pin_is_a_false_not_an_error() {
        // A caller must not be able to tell "wrong PIN" from "no such user" by error type.
        let stored = hash("8317", &salt()).expect("hashes");
        assert!(verify("0000", &stored).is_ok());
    }

    #[test]
    fn the_stored_hash_is_a_phc_string_and_contains_no_pin() {
        let stored = hash("8317", &salt()).expect("hashes");

        assert!(stored.starts_with("$argon2id$v=19$"));
        assert!(!stored.contains("8317"));
    }

    #[test]
    fn a_corrupt_stored_hash_is_an_error_not_a_silent_false() {
        // Silently returning false would turn a database problem into "everyone's PIN is wrong",
        // which reads like user error and gets diagnosed for hours.
        assert_eq!(verify("8317", "not a hash"), Err(PinError::CorruptHash));
    }

    #[test]
    fn a_short_pin_is_refused() {
        assert_eq!(validate("123"), Err(PinError::Length { length: 3 }));
        assert_eq!(validate(""), Err(PinError::Length { length: 0 }));
    }

    #[test]
    fn a_long_pin_is_refused() {
        assert_eq!(validate("123456789"), Err(PinError::Length { length: 9 }));
    }

    #[test]
    fn a_non_numeric_pin_is_refused() {
        // The terminal shows a numeric keypad; anything else got in some other way.
        assert_eq!(validate("12a4"), Err(PinError::NotNumeric));
        assert_eq!(validate("১২৩৪"), Err(PinError::NotNumeric));
    }

    #[test]
    fn the_pins_people_actually_pick_are_refused() {
        for weak in ["0000", "1111", "9999", "1234", "4321", "012345", "654321"] {
            assert_eq!(validate(weak), Err(PinError::Guessable), "accepted {weak}");
        }
    }

    #[test]
    fn a_pin_that_merely_contains_a_run_is_fine() {
        // Only whole-run PINs are refused. Rejecting anything containing "123" would leave a
        // cashier guessing at what the rule is.
        assert_eq!(validate("1237"), Ok(()));
        assert_eq!(validate("8317"), Ok(()));
    }

    #[test]
    fn a_wrapping_sequence_is_not_treated_as_a_run() {
        assert_eq!(validate("9012"), Ok(()));
    }

    #[test]
    fn hashing_refuses_a_pin_validation_would_refuse() {
        // Otherwise the rule holds only wherever someone remembered to call `validate` first.
        assert_eq!(hash("0000", &salt()), Err(PinError::Guessable));
    }

    #[test]
    fn a_weaker_stored_hash_is_flagged_for_rehash() {
        let weak = Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(8, 1, 1, None).expect("valid params"),
        )
        .hash_password(b"8317", salt().as_salt())
        .expect("hashes")
        .to_string();

        assert!(needs_rehash(&weak));
        assert!(
            verify("8317", &weak).expect("still verifies"),
            "not locked out by a cost change"
        );
    }

    #[test]
    fn a_current_hash_does_not_need_rehashing() {
        let stored = hash("8317", &salt()).expect("hashes");
        assert!(!needs_rehash(&stored));
    }

    #[test]
    fn an_unreadable_hash_needs_rehashing() {
        assert!(needs_rehash("not a hash"));
    }
}
