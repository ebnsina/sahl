//! Enrolling this till.
//!
//! The keypair is generated here and the private half never leaves. The server only ever learns a
//! public key, so a full server compromise still cannot forge this device's sales.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::SigningKey;
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::terminal::DeviceIdentity;

#[derive(Debug, Error)]
pub enum EnrollmentError {
    #[error("could not gather secure randomness: {0}")]
    Entropy(String),

    #[error("could not reach the server: {0}")]
    Transport(String),

    /// Deliberately uninformative — the server does not say why, on purpose.
    #[error("the server refused this enrollment; check the token has not expired or been used")]
    Refused,

    #[error("the server's reply was not understood")]
    MalformedReply,

    #[error("could not store the device credentials: {0}")]
    Storage(String),

    #[error("this device is not enrolled")]
    NotEnrolled,
}

/// What a device keeps after enrolling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub identity: DeviceIdentity,
    /// Base64url Ed25519 signing key.
    ///
    /// TODO(P2): move to the OS keychain via Tauri. A file with 0600 permissions is honest interim
    /// protection against another user on the same machine, and no protection at all against
    /// someone holding the disk — which is why revocation is server-side and immediate.
    secret_key: String,
}

impl Credentials {
    /// # Errors
    /// [`EnrollmentError::MalformedReply`] if the stored key is not a valid Ed25519 key.
    pub fn signing_key(&self) -> Result<SigningKey, EnrollmentError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(&self.secret_key)
            .map_err(|_| EnrollmentError::MalformedReply)?;
        let sized: [u8; 32] = bytes
            .try_into()
            .map_err(|_| EnrollmentError::MalformedReply)?;
        Ok(SigningKey::from_bytes(&sized))
    }
}

/// Where the credentials live inside the app data directory.
#[must_use]
pub fn credentials_path(data_dir: &Path) -> PathBuf {
    data_dir.join("device.json")
}

/// Load credentials, if this till has been enrolled.
///
/// # Errors
/// [`EnrollmentError`] if the file exists but cannot be read or parsed.
pub fn load(data_dir: &Path) -> Result<Option<Credentials>, EnrollmentError> {
    let path = credentials_path(data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| EnrollmentError::Storage(error.to_string()))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| EnrollmentError::Storage(error.to_string()))
}

/// Generate a keypair and redeem `token` against the server.
///
/// The private key is written to disk **before** the terminal starts trading, and the public half
/// is all that crosses the wire.
///
/// # Errors
/// [`EnrollmentError`] on entropy failure, transport failure, refusal, or storage failure.
pub fn enroll(
    base_url: &str,
    token: &str,
    label: &str,
    data_dir: &Path,
) -> Result<Credentials, EnrollmentError> {
    let mut seed = [0u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut seed)
        .map_err(|error| EnrollmentError::Entropy(error.to_string()))?;
    let key = SigningKey::from_bytes(&seed);

    let body = serde_json::json!({
        "token": token.trim(),
        "public_key": URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes()),
        "label": label,
    });

    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| EnrollmentError::Transport(error.to_string()))?
        .post(format!(
            "{}/v1/devices/enroll",
            base_url.trim_end_matches('/')
        ))
        .json(&body)
        .send()
        .map_err(|error| EnrollmentError::Transport(error.to_string()))?;

    if !response.status().is_success() {
        return Err(EnrollmentError::Refused);
    }

    #[derive(Deserialize)]
    struct Reply {
        device_id: Uuid,
        tenant_id: Uuid,
        outlet_id: Uuid,
    }

    let reply: Reply = response
        .json()
        .map_err(|_| EnrollmentError::MalformedReply)?;

    let credentials = Credentials {
        identity: DeviceIdentity {
            tenant_id: reply.tenant_id,
            outlet_id: reply.outlet_id,
            device_id: reply.device_id,
        },
        secret_key: URL_SAFE_NO_PAD.encode(key.to_bytes()),
    };

    store(data_dir, &credentials)?;
    Ok(credentials)
}

/// Persist credentials with owner-only permissions.
fn store(data_dir: &Path, credentials: &Credentials) -> Result<(), EnrollmentError> {
    std::fs::create_dir_all(data_dir)
        .map_err(|error| EnrollmentError::Storage(error.to_string()))?;
    let path = credentials_path(data_dir);
    let text = serde_json::to_string_pretty(credentials)
        .map_err(|error| EnrollmentError::Storage(error.to_string()))?;
    std::fs::write(&path, text).map_err(|error| EnrollmentError::Storage(error.to_string()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 0600. Weak on its own — anyone with the disk can read it — but it does stop another user
        // account on a shared shop PC, which is the realistic local threat.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| EnrollmentError::Storage(error.to_string()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sahl-enroll-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn sample() -> Credentials {
        Credentials {
            identity: DeviceIdentity {
                tenant_id: Uuid::from_u128(1),
                outlet_id: Uuid::from_u128(2),
                device_id: Uuid::from_u128(3),
            },
            secret_key: URL_SAFE_NO_PAD.encode([9u8; 32]),
        }
    }

    #[test]
    fn an_unenrolled_device_reports_no_credentials() {
        let dir = temp_dir("absent");
        std::fs::remove_file(credentials_path(&dir)).ok();
        assert!(load(&dir).expect("loads").is_none());
    }

    #[test]
    fn credentials_round_trip_through_disk() {
        let dir = temp_dir("roundtrip");
        store(&dir, &sample()).expect("stores");

        let loaded = load(&dir).expect("loads").expect("present");
        assert_eq!(loaded.identity.device_id, Uuid::from_u128(3));
        assert_eq!(
            loaded.signing_key().expect("key").to_bytes(),
            [9u8; 32],
            "the private key survives intact"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("perms");
        store(&dir, &sample()).expect("stores");

        let mode = std::fs::metadata(credentials_path(&dir))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "no group or other access");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_key_is_reported_rather_than_used() {
        let mut broken = sample();
        broken.secret_key = "not-base64!!".to_owned();
        assert!(broken.signing_key().is_err());
    }
}
