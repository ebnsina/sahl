//! Request signing and verification for enrolled terminals.
//!
//! Every request from a terminal carries an Ed25519 signature made with a key whose private half was
//! generated on the device and never leaves it. The server only ever holds the public half, so a
//! compromise of the server database cannot forge a terminal's traffic — which matters more here
//! than in most systems, because those requests are the merchant's financial record.
//!
//! Everything in this module is a pure function over bytes. That keeps the security-critical logic
//! testable without a database, a network, or a clock.

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

/// Prefix binding a signature to this scheme and version.
///
/// Domain separation: without it, bytes signed for some future purpose could be replayed as a
/// request signature. Bumping the version invalidates every old signature deliberately.
const SIGNING_DOMAIN: &str = "sahl-request-v1";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SignatureError {
    #[error("device {device_id} has been revoked")]
    Revoked { device_id: Uuid },

    #[error("stored public key for device {device_id} is malformed")]
    MalformedPublicKey { device_id: Uuid },

    #[error("signature is not 64 bytes")]
    MalformedSignature,

    #[error(
        "request timestamp is {skew_seconds}s from server time, beyond the {allowed_seconds}s window"
    )]
    TimestampOutOfWindow {
        skew_seconds: i64,
        allowed_seconds: i64,
    },

    #[error("signature does not verify for device {device_id}")]
    Rejected { device_id: Uuid },
}

/// The parts of a request that are covered by its signature.
#[derive(Debug, Clone, Copy)]
pub struct SignedRequest<'a> {
    pub device_id: Uuid,
    pub method: &'a str,
    pub path: &'a str,
    /// Milliseconds since the Unix epoch, as claimed by the device.
    pub timestamp_millis: i64,
    pub body: &'a [u8],
}

impl SignedRequest<'_> {
    /// The exact bytes signed and verified.
    ///
    /// Newline-delimited with a fixed field order, and every field length-determined by its own
    /// content, so no two distinct requests can produce the same bytes. Including `device_id` binds
    /// the signature to one terminal: a captured request cannot be replayed as if from another.
    ///
    /// The body is committed to by digest rather than inline, so a large sync batch does not have to
    /// be held in memory twice to verify it.
    #[must_use]
    pub fn signing_payload(&self) -> Vec<u8> {
        let body_digest = hex::encode(Sha256::digest(self.body));
        format!(
            "{SIGNING_DOMAIN}\n{}\n{}\n{}\n{}\n{}",
            self.method.to_ascii_uppercase(),
            self.path,
            self.device_id,
            self.timestamp_millis,
            body_digest
        )
        .into_bytes()
    }
}

/// A device's stored credentials, as loaded from the database.
#[derive(Debug, Clone, Copy)]
pub struct DeviceCredentials {
    pub device_id: Uuid,
    pub public_key: [u8; 32],
    pub revoked: bool,
}

/// Verify a request's signature.
///
/// Checks in a deliberate order — revocation, then freshness, then cryptography. Revocation first so
/// a stolen terminal is refused without spending a signature verification on it, and freshness
/// before verification for the same reason.
///
/// # Errors
/// The specific [`SignatureError`]. Callers should return a single opaque 401 to the client rather
/// than the variant: telling an attacker *why* a request failed helps them.
pub fn verify_request(
    request: &SignedRequest<'_>,
    credentials: &DeviceCredentials,
    signature_bytes: &[u8],
    server_now_millis: i64,
    max_skew_seconds: i64,
) -> Result<(), SignatureError> {
    if credentials.revoked {
        return Err(SignatureError::Revoked {
            device_id: credentials.device_id,
        });
    }

    // Absolute skew, so a device whose clock runs fast is rejected as firmly as a replayed request
    // from the past. `saturating_sub` because a malicious client can send i64::MIN.
    let skew_millis = server_now_millis
        .saturating_sub(request.timestamp_millis)
        .saturating_abs();
    let allowed_millis = max_skew_seconds.saturating_mul(1_000);
    if skew_millis > allowed_millis {
        return Err(SignatureError::TimestampOutOfWindow {
            skew_seconds: skew_millis.saturating_div(1_000),
            allowed_seconds: max_skew_seconds,
        });
    }

    let verifying_key = VerifyingKey::from_bytes(&credentials.public_key).map_err(|_| {
        SignatureError::MalformedPublicKey {
            device_id: credentials.device_id,
        }
    })?;

    let signature_array: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| SignatureError::MalformedSignature)?;
    let signature = Signature::from_bytes(&signature_array);

    // `verify_strict` rejects small-order and non-canonical public keys, which the permissive
    // `verify` accepts. For a signature that authenticates financial records, strict is the only
    // defensible choice.
    verifying_key
        .verify_strict(&request.signing_payload(), &signature)
        .map_err(|_| SignatureError::Rejected {
            device_id: credentials.device_id,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const NOW: i64 = 1_753_000_000_000;
    const SKEW_SECONDS: i64 = 300;

    fn device_id() -> Uuid {
        Uuid::from_u128(0xD3)
    }

    fn keypair() -> SigningKey {
        // Fixed seed: these tests must be deterministic, and this key never leaves the test binary.
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn credentials(signing_key: &SigningKey, revoked: bool) -> DeviceCredentials {
        DeviceCredentials {
            device_id: device_id(),
            public_key: signing_key.verifying_key().to_bytes(),
            revoked,
        }
    }

    fn request<'a>(body: &'a [u8], timestamp: i64) -> SignedRequest<'a> {
        SignedRequest {
            device_id: device_id(),
            method: "POST",
            path: "/v1/sync/push",
            timestamp_millis: timestamp,
            body,
        }
    }

    fn sign(key: &SigningKey, request: &SignedRequest<'_>) -> Vec<u8> {
        key.sign(&request.signing_payload()).to_bytes().to_vec()
    }

    #[test]
    fn a_genuine_request_verifies() {
        let key = keypair();
        let req = request(b"{\"events\":[]}", NOW);
        let signature = sign(&key, &req);

        assert_eq!(
            verify_request(
                &req,
                &credentials(&key, false),
                &signature,
                NOW,
                SKEW_SECONDS
            ),
            Ok(())
        );
    }

    #[test]
    fn a_tampered_body_is_rejected() {
        // The whole point: an intercepted sync batch cannot have events added or removed.
        let key = keypair();
        let original = request(b"{\"events\":[1]}", NOW);
        let signature = sign(&key, &original);

        let tampered = request(b"{\"events\":[1,2]}", NOW);
        assert_eq!(
            verify_request(
                &tampered,
                &credentials(&key, false),
                &signature,
                NOW,
                SKEW_SECONDS
            ),
            Err(SignatureError::Rejected {
                device_id: device_id()
            })
        );
    }

    #[test]
    fn a_request_replayed_against_another_endpoint_is_rejected() {
        let key = keypair();
        let signed_for_push = request(b"{}", NOW);
        let signature = sign(&key, &signed_for_push);

        let mut moved = signed_for_push;
        moved.path = "/v1/devices/revoke";

        assert!(
            verify_request(
                &moved,
                &credentials(&key, false),
                &signature,
                NOW,
                SKEW_SECONDS
            )
            .is_err()
        );
    }

    #[test]
    fn a_request_replayed_as_another_device_is_rejected() {
        // device_id is inside the signed payload, so a captured request is bound to one terminal.
        let key = keypair();
        let original = request(b"{}", NOW);
        let signature = sign(&key, &original);

        let mut impersonated = original;
        impersonated.device_id = Uuid::from_u128(0xBEEF);

        assert!(
            verify_request(
                &impersonated,
                &credentials(&key, false),
                &signature,
                NOW,
                SKEW_SECONDS
            )
            .is_err()
        );
    }

    #[test]
    fn a_stale_request_is_rejected_before_any_crypto_runs() {
        let key = keypair();
        let stale = request(b"{}", NOW - 600_000); // ten minutes old
        let signature = sign(&key, &stale);

        assert!(matches!(
            verify_request(
                &stale,
                &credentials(&key, false),
                &signature,
                NOW,
                SKEW_SECONDS
            ),
            Err(SignatureError::TimestampOutOfWindow { .. })
        ));
    }

    #[test]
    fn a_future_dated_request_is_rejected_too() {
        // A device whose clock runs fast is as much of a problem as one running slow.
        let key = keypair();
        let ahead = request(b"{}", NOW + 600_000);
        let signature = sign(&key, &ahead);

        assert!(matches!(
            verify_request(
                &ahead,
                &credentials(&key, false),
                &signature,
                NOW,
                SKEW_SECONDS
            ),
            Err(SignatureError::TimestampOutOfWindow { .. })
        ));
    }

    #[test]
    fn an_extreme_timestamp_does_not_overflow() {
        // A malicious client controls this value entirely.
        let key = keypair();
        let absurd = request(b"{}", i64::MIN);
        let signature = sign(&key, &absurd);

        assert!(matches!(
            verify_request(
                &absurd,
                &credentials(&key, false),
                &signature,
                NOW,
                SKEW_SECONDS
            ),
            Err(SignatureError::TimestampOutOfWindow { .. })
        ));
    }

    #[test]
    fn a_revoked_device_is_refused_even_with_a_valid_signature() {
        // Revocation has to be absolute: this is what stops a stolen terminal.
        let key = keypair();
        let req = request(b"{}", NOW);
        let signature = sign(&key, &req);

        assert_eq!(
            verify_request(
                &req,
                &credentials(&key, true),
                &signature,
                NOW,
                SKEW_SECONDS
            ),
            Err(SignatureError::Revoked {
                device_id: device_id()
            })
        );
    }

    #[test]
    fn another_devices_signature_is_rejected() {
        let key = keypair();
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let req = request(b"{}", NOW);
        let signature = sign(&other, &req);

        assert!(
            verify_request(
                &req,
                &credentials(&key, false),
                &signature,
                NOW,
                SKEW_SECONDS
            )
            .is_err()
        );
    }

    #[test]
    fn a_malformed_signature_is_reported_not_panicked_on() {
        let key = keypair();
        let req = request(b"{}", NOW);

        assert_eq!(
            verify_request(
                &req,
                &credentials(&key, false),
                b"too short",
                NOW,
                SKEW_SECONDS
            ),
            Err(SignatureError::MalformedSignature)
        );
    }

    #[test]
    fn the_signing_payload_is_domain_separated_and_stable() {
        let req = request(b"{}", NOW);
        let payload = String::from_utf8(req.signing_payload()).expect("utf-8");

        assert!(payload.starts_with("sahl-request-v1\n"));
        assert!(payload.contains("POST\n/v1/sync/push\n"));
        // The body is committed to by digest, not inlined.
        assert!(payload.contains(&hex::encode(Sha256::digest(b"{}"))));
    }

    #[test]
    fn the_method_is_normalised_so_case_cannot_split_a_signature() {
        let mut lower = request(b"{}", NOW);
        lower.method = "post";
        let upper = request(b"{}", NOW);

        assert_eq!(lower.signing_payload(), upper.signing_payload());
    }
}
