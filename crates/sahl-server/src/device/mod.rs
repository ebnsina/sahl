//! Device identity: enrollment, and per-request signature verification.
//!
//! A terminal is a first-class principal here, not a session. It holds its own keypair, signs every
//! request, and can be revoked server-side the moment it is lost — which is the realistic threat in
//! this market, where the "device" is a cheap tablet sitting on a counter all day.

pub mod enrollment;
pub mod signing;

pub use enrollment::{
    EnrollmentError, MintedToken, StoredToken, check_token_usable, digest_token, digests_match,
    mint_token, parse_public_key,
};
pub use signing::{DeviceCredentials, SignatureError, SignedRequest, verify_request};
