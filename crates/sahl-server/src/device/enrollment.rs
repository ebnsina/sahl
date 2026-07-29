//! Device enrollment tokens.
//!
//! An owner generates a token in the dashboard and types it into a new terminal. The terminal
//! generates its own Ed25519 keypair, keeps the private half in the OS keychain, and presents the
//! token together with its public key. The server binds the two and burns the token.
//!
//! Three properties make this safe enough for a credential a human reads off a screen:
//!
//! - **The private key never transits.** The server only ever learns a public key, so it cannot
//!   forge a device's events even if fully compromised.
//! - **Only a digest is stored.** A leaked database backup does not let anyone enrol a terminal; the
//!   plaintext token exists exactly once, in the response that created it.
//! - **Comparison is constant-time.** Token lookup is by digest equality, and a timing-variable
//!   comparison would leak the digest byte by byte to anyone able to measure it.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

/// Bytes of entropy in a token.
///
/// 32 bytes is well beyond guessable, and base64url-encodes to 43 characters — long, but this is
/// copied or scanned once during setup, not typed daily.
const TOKEN_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EnrollmentError {
    #[error("enrollment token has expired")]
    Expired,

    #[error("enrollment token has already been used")]
    AlreadyConsumed,

    #[error("public key must be exactly 32 bytes")]
    MalformedPublicKey,

    #[error("could not gather secure randomness: {0}")]
    Entropy(String),
}

/// A freshly minted token: the plaintext to hand out once, and the digest to store.
#[derive(Debug, Clone)]
pub struct MintedToken {
    /// Shown to the operator exactly once. Never logged, never persisted.
    pub plaintext: String,
    /// SHA-256 of the plaintext. This is what goes in the database.
    pub digest: [u8; 32],
}

/// Mint a new enrollment token.
///
/// # Errors
/// [`EnrollmentError::Entropy`] if the OS random source fails — which must abort enrollment rather
/// than fall back to a weaker source.
pub fn mint_token() -> Result<MintedToken, EnrollmentError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|error| EnrollmentError::Entropy(error.to_string()))?;

    let plaintext = URL_SAFE_NO_PAD.encode(bytes);
    let digest = digest_token(&plaintext);
    Ok(MintedToken { plaintext, digest })
}

/// Digest a presented token so it can be looked up against stored rows.
#[must_use]
pub fn digest_token(plaintext: &str) -> [u8; 32] {
    Sha256::digest(plaintext.trim().as_bytes()).into()
}

/// Compare two digests without leaking their contents through timing.
#[must_use]
pub fn digests_match(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.ct_eq(right).into()
}

/// What the database knows about a stored token.
#[derive(Debug, Clone, Copy)]
pub struct StoredToken {
    pub expires_at_millis: i64,
    pub consumed: bool,
}

/// Check a stored token is still usable.
///
/// Consumption is checked before expiry so that re-presenting a used token always reports the same
/// thing, regardless of how long ago it was used.
///
/// # Errors
/// [`EnrollmentError::AlreadyConsumed`] or [`EnrollmentError::Expired`].
pub fn check_token_usable(token: &StoredToken, now_millis: i64) -> Result<(), EnrollmentError> {
    if token.consumed {
        return Err(EnrollmentError::AlreadyConsumed);
    }
    if now_millis >= token.expires_at_millis {
        return Err(EnrollmentError::Expired);
    }
    Ok(())
}

/// Decode and validate a device's presented public key.
///
/// # Errors
/// [`EnrollmentError::MalformedPublicKey`] if it is not 32 base64url bytes forming a valid Ed25519
/// key. Weak and small-order keys are rejected here rather than at first use.
pub fn parse_public_key(encoded: &str) -> Result<[u8; 32], EnrollmentError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded.trim())
        .map_err(|_| EnrollmentError::MalformedPublicKey)?;
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| EnrollmentError::MalformedPublicKey)?;

    let key = ed25519_dalek::VerifyingKey::from_bytes(&array)
        .map_err(|_| EnrollmentError::MalformedPublicKey)?;

    // `from_bytes` alone is not enough: it happily accepts small-order points such as all-zeroes.
    // Those keys make signatures forgeable by anyone, so a terminal presenting one — whether through
    // a broken RNG or deliberately — must never reach the device table. `verify_strict` would catch
    // it later at every request, but refusing at enrollment means the bad device never exists.
    if key.is_weak() {
        return Err(EnrollmentError::MalformedPublicKey);
    }
    Ok(array)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_753_000_000_000;

    #[test]
    fn a_minted_token_digests_to_its_stored_form() {
        let minted = mint_token().expect("entropy available");
        assert_eq!(digest_token(&minted.plaintext), minted.digest);
    }

    #[test]
    fn tokens_are_unique_across_mints() {
        let a = mint_token().expect("entropy available");
        let b = mint_token().expect("entropy available");
        assert_ne!(a.plaintext, b.plaintext);
        assert_ne!(a.digest, b.digest);
    }

    #[test]
    fn a_token_carries_full_entropy() {
        let minted = mint_token().expect("entropy available");
        // 32 bytes base64url-encodes to 43 characters with no padding.
        assert_eq!(minted.plaintext.len(), 43);
        assert!(!minted.plaintext.contains('='));
    }

    #[test]
    fn surrounding_whitespace_is_tolerated_when_typed_in() {
        // Operators paste these; a trailing newline should not fail an enrolment.
        let minted = mint_token().expect("entropy available");
        let padded = format!("  {}\n", minted.plaintext);
        assert!(digests_match(&digest_token(&padded), &minted.digest));
    }

    #[test]
    fn a_wrong_token_does_not_match() {
        let a = mint_token().expect("entropy available");
        let b = mint_token().expect("entropy available");
        assert!(!digests_match(&a.digest, &b.digest));
    }

    #[test]
    fn a_fresh_token_is_usable() {
        let token = StoredToken {
            expires_at_millis: NOW + 60_000,
            consumed: false,
        };
        assert_eq!(check_token_usable(&token, NOW), Ok(()));
    }

    #[test]
    fn an_expired_token_is_refused() {
        let token = StoredToken {
            expires_at_millis: NOW,
            consumed: false,
        };
        // Expiry is inclusive: at the exact millisecond, it is already gone.
        assert_eq!(
            check_token_usable(&token, NOW),
            Err(EnrollmentError::Expired)
        );
    }

    #[test]
    fn a_used_token_cannot_enrol_a_second_device() {
        // Single-use is what stops one leaked token enrolling a fleet of rogue terminals.
        let token = StoredToken {
            expires_at_millis: NOW + 60_000,
            consumed: true,
        };
        assert_eq!(
            check_token_usable(&token, NOW),
            Err(EnrollmentError::AlreadyConsumed)
        );
    }

    #[test]
    fn a_used_token_reports_consumption_even_after_expiry() {
        let token = StoredToken {
            expires_at_millis: NOW - 60_000,
            consumed: true,
        };
        assert_eq!(
            check_token_usable(&token, NOW),
            Err(EnrollmentError::AlreadyConsumed)
        );
    }

    #[test]
    fn a_real_public_key_round_trips() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
        let encoded = URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes());
        assert_eq!(
            parse_public_key(&encoded),
            Ok(key.verifying_key().to_bytes())
        );
    }

    #[test]
    fn a_wrong_length_key_is_refused() {
        assert_eq!(
            parse_public_key(&URL_SAFE_NO_PAD.encode([1u8; 31])),
            Err(EnrollmentError::MalformedPublicKey)
        );
    }

    #[test]
    fn non_base64_input_is_refused() {
        assert_eq!(
            parse_public_key("not valid base64!!!"),
            Err(EnrollmentError::MalformedPublicKey)
        );
    }

    #[test]
    fn an_all_zero_key_is_refused_at_enrolment() {
        // A small-order key would let anyone forge signatures for that device. Catching it here
        // means an unusable terminal can never reach the device table at all.
        assert_eq!(
            parse_public_key(&URL_SAFE_NO_PAD.encode([0u8; 32])),
            Err(EnrollmentError::MalformedPublicKey)
        );
    }
}
