use core::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use super::error::EventError;

/// A SHA-256 digest linking one event to the one before it.
///
/// SHA-256 rather than something faster because this chain is a fiscal artefact, not just an
/// integrity check: ZATCA's invoice chain mandates SHA-256, and using the same primitive throughout
/// means the fiscal document chain and the sync event chain share one implementation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventHash([u8; Self::LEN]);

impl EventHash {
    /// Digest length in bytes.
    pub const LEN: usize = 32;

    /// The chain's starting point: all zeroes.
    ///
    /// Every device's first event links to this, which is what makes "is this the true first
    /// event?" a checkable question rather than an assumption.
    pub const GENESIS: Self = Self([0u8; Self::LEN]);

    /// Compute the digest of `bytes`.
    #[must_use]
    pub fn digest(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }

    /// Construct from raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    /// Lowercase hex, the form stored in SQLite and Postgres.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse from lowercase or uppercase hex.
    ///
    /// # Errors
    /// [`EventError::MalformedHash`] if the input is not exactly 64 hex characters.
    pub fn from_hex(value: &str) -> Result<Self, EventError> {
        let bytes = hex::decode(value).map_err(|_| EventError::MalformedHash {
            value: value.to_owned(),
        })?;
        let sized: [u8; Self::LEN] = bytes.try_into().map_err(|_| EventError::MalformedHash {
            value: value.to_owned(),
        })?;
        Ok(Self(sized))
    }

    /// Whether this is the genesis hash.
    #[must_use]
    pub fn is_genesis(&self) -> bool {
        *self == Self::GENESIS
    }
}

impl fmt::Display for EventHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for EventHash {
    /// Abbreviated, because a full digest in a log line buries everything around it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex = self.to_hex();
        let head = hex.get(..12).unwrap_or(&hex);
        write!(f, "EventHash({head}…)")
    }
}

impl Serialize for EventHash {
    /// Serialises as hex rather than a byte array so that the canonical JSON used for hashing is
    /// unambiguous and human-readable in a debugging session.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for EventHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_is_all_zeroes_and_recognises_itself() {
        assert!(EventHash::GENESIS.is_genesis());
        assert_eq!(EventHash::GENESIS.to_hex(), "0".repeat(64));
    }

    #[test]
    fn digests_are_stable_and_match_the_known_sha256_vector() {
        // The canonical empty-string SHA-256. If this ever changes, every chain in the field breaks.
        assert_eq!(
            EventHash::digest(b"").to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn distinct_inputs_produce_distinct_digests() {
        assert_ne!(EventHash::digest(b"sale"), EventHash::digest(b"sale "));
    }

    #[test]
    fn hex_round_trips() {
        let hash = EventHash::digest(b"an event");
        assert_eq!(EventHash::from_hex(&hash.to_hex()), Ok(hash));
    }

    #[test]
    fn malformed_hex_is_rejected_rather_than_padded() {
        assert!(EventHash::from_hex("abc").is_err());
        assert!(EventHash::from_hex("zz".repeat(32).as_str()).is_err());
        assert!(EventHash::from_hex(&"ab".repeat(33)).is_err());
    }

    #[test]
    fn json_round_trips_through_hex() {
        let hash = EventHash::digest(b"payload");
        let encoded = serde_json::to_string(&hash).expect("serialises");
        assert_eq!(encoded, format!("\"{}\"", hash.to_hex()));
        assert_eq!(
            serde_json::from_str::<EventHash>(&encoded).expect("deserialises"),
            hash
        );
    }

    #[test]
    fn debug_is_abbreviated_so_logs_stay_readable() {
        let rendered = format!("{:?}", EventHash::digest(b""));
        assert_eq!(rendered, "EventHash(e3b0c44298fc…)");
    }
}
