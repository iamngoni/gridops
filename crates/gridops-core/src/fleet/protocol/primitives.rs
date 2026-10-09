//! Bounded fleet wire primitives and payload parsing.
//! Byte limits are checked before JSON deserialization. Browser numbers are
//! restricted to exact JavaScript integers instead of rounding database values.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_DATABASE_INTEGER: u64 = i64::MAX as u64;
pub const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
pub const DEFAULT_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_BATCH_BYTES: usize = 256 * 1024;
pub const MAX_CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_EVENT_ITEMS: usize = 100;
pub const MAX_CURSOR_BYTES: usize = 4096;
pub const MAX_RESOURCE_DOMAIN_DEPTH: usize = 64;

/// A SHA-256 digest uses one canonical base64url encoding across fleet streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest([u8; 32]);
impl Sha256Digest {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }
}
impl TryFrom<String> for Sha256Digest {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 43 {
            return Err(ContractError::InvalidText);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&value)
            .map_err(|_| ContractError::InvalidText)?;
        if URL_SAFE_NO_PAD.encode(&bytes) != value {
            return Err(ContractError::InvalidText);
        }
        Ok(Self(
            bytes.try_into().map_err(|_| ContractError::InvalidText)?,
        ))
    }
}
impl From<Sha256Digest> for String {
    fn from(value: Sha256Digest) -> Self {
        URL_SAFE_NO_PAD.encode(value.0)
    }
}

/// Authorized log content is bounded in bytes and excluded from debug output.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LogText(String);
impl LogText {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for LogText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LogText([redacted])")
    }
}
impl TryFrom<String> for LogText {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > MAX_CHUNK_BYTES {
            return Err(ContractError::PayloadTooLarge);
        }
        Ok(Self(value))
    }
}
impl From<LogText> for String {
    fn from(value: LogText) -> Self {
        value.0
    }
}

/// An exact, bounded integer whose constructor and deserializer agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoundedInteger<const MIN: u64, const MAX: u64>(u64);

impl<const MIN: u64, const MAX: u64> BoundedInteger<MIN, MAX> {
    pub fn new(value: u64) -> Result<Self, ContractError> {
        if !(MIN..=MAX).contains(&value) {
            return Err(ContractError::IntegerRange);
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl<const MIN: u64, const MAX: u64> Serialize for BoundedInteger<MIN, MAX> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0)
    }
}

impl<'de, const MIN: u64, const MAX: u64> Deserialize<'de> for BoundedInteger<MIN, MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

pub type BrowserUint53 = BoundedInteger<0, MAX_SAFE_INTEGER>;
pub type BrowserCounter = BoundedInteger<1, MAX_SAFE_INTEGER>;
pub type ByteOffset = BoundedInteger<0, MAX_DATABASE_INTEGER>;
pub type ProtocolVersion = BoundedInteger<1, 1>;
pub type PollDurationMs = BoundedInteger<1, 25_000>;
pub type AuthorityDurationMs = BoundedInteger<1, 90_000>;
pub type PageSize = BoundedInteger<1, 100>;
pub type EnrollmentDurationMs = BoundedInteger<60_000, 3_600_000>;
pub type PreparationDurationMs = BoundedInteger<1, 3_600_000>;
pub type AttemptNumber = BoundedInteger<1, 10>;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContractError {
    #[error("integer is outside the contract range")]
    IntegerRange,
    #[error("payload exceeds the contract limit")]
    PayloadTooLarge,
    #[error("invalid protocol JSON")]
    InvalidJson,
    #[error("timestamp must be UTC RFC3339 with millisecond precision")]
    InvalidTimestamp,
    #[error("invalid bounded text")]
    InvalidText,
    #[error("unsafe relative path")]
    UnsafePath,
}

/// Strict UTC time, retaining exactly the precision stored in the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WireTimestamp(DateTime<Utc>);

impl WireTimestamp {
    pub fn from_millis(value: i64) -> Result<Self, ContractError> {
        DateTime::from_timestamp_millis(value)
            .filter(|time| (0..=9999).contains(&time.year()))
            .map(Self)
            .ok_or(ContractError::InvalidTimestamp)
    }

    pub const fn millis(self) -> i64 {
        self.0.timestamp_millis()
    }
}

impl TryFrom<String> for WireTimestamp {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > 32
            || !value.contains('T')
            || !(value.ends_with('Z') || value.ends_with("+00:00") || value.ends_with("-00:00"))
        {
            return Err(ContractError::InvalidTimestamp);
        }
        let parsed =
            DateTime::parse_from_rfc3339(&value).map_err(|_| ContractError::InvalidTimestamp)?;
        if parsed.offset().local_minus_utc() != 0
            || !(0..=9999).contains(&parsed.year())
            || parsed.timestamp_subsec_nanos() >= 1_000_000_000
            || parsed.timestamp_subsec_nanos() % 1_000_000 != 0
        {
            return Err(ContractError::InvalidTimestamp);
        }
        Ok(Self(parsed.with_timezone(&Utc)))
    }
}

/// Sandbox arguments are content, not identity strings. Empty and multiline
/// arguments are legitimate; NUL cannot be passed to an OS process.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SandboxArgument(String);
impl SandboxArgument {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for SandboxArgument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SandboxArgument([redacted])")
    }
}
impl TryFrom<String> for SandboxArgument {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > 16_384 || value.contains('\0') {
            return Err(ContractError::InvalidText);
        }
        Ok(Self(value))
    }
}
impl From<SandboxArgument> for String {
    fn from(value: SandboxArgument) -> Self {
        value.0
    }
}

/// Executable name/path inside the owned sandbox, never on the physical host.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SandboxProgram(String);
impl SandboxProgram {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for SandboxProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SandboxProgram([redacted])")
    }
}
impl TryFrom<String> for SandboxProgram {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || value.len() > 1024 || value.contains('\0') {
            return Err(ContractError::InvalidText);
        }
        Ok(Self(value))
    }
}
impl From<SandboxProgram> for String {
    fn from(value: SandboxProgram) -> Self {
        value.0
    }
}

/// Runtime image/release references retain the established 300-character limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ImageReference(String);
impl ImageReference {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for ImageReference {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || value.chars().count() > 300 || value.chars().any(char::is_control) {
            return Err(ContractError::InvalidText);
        }
        Ok(Self(value))
    }
}
impl From<ImageReference> for String {
    fn from(value: ImageReference) -> Self {
        value.0
    }
}

impl From<WireTimestamp> for String {
    fn from(value: WireTimestamp) -> Self {
        value.0.to_rfc3339_opts(SecondsFormat::Millis, true)
    }
}

/// Header-only idempotency key; a browser body cannot select an actor scope.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct IdempotencyKey(String);

impl std::fmt::Debug for IdempotencyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IdempotencyKey([redacted])")
    }
}

impl IdempotencyKey {
    pub fn parse(value: impl Into<String>) -> Result<Self, ContractError> {
        Self::try_from(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for IdempotencyKey {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        {
            return Err(ContractError::InvalidText);
        }
        Ok(Self(value))
    }
}

impl From<IdempotencyKey> for String {
    fn from(value: IdempotencyKey) -> Self {
        value.0
    }
}

/// Portable path inside an owned sandbox. Runtime access must also prevent
/// symlinks from escaping its root; lexical validation cannot establish that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SandboxPath(String);

impl SandboxPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SandboxPath {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 1024
            || value
                .chars()
                .any(|ch| ch.is_control() || ch == '\\' || ch == ':')
            || value
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(ContractError::UnsafePath);
        }
        Ok(Self(value))
    }
}

impl From<SandboxPath> for String {
    fn from(value: SandboxPath) -> Self {
        value.0
    }
}

/// Reject oversized bodies before allocating a deserialized object graph.
pub fn parse_json<T: DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T, ContractError> {
    if limit == 0 || limit > MAX_JSON_BYTES || bytes.len() > limit {
        return Err(ContractError::PayloadTooLarge);
    }
    serde_json::from_slice(bytes).map_err(|_| ContractError::InvalidJson)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_boundaries_are_exact() -> Result<(), ContractError> {
        assert_eq!(
            BrowserUint53::new(MAX_SAFE_INTEGER)?.get(),
            MAX_SAFE_INTEGER
        );
        assert_eq!(
            BrowserUint53::new(MAX_SAFE_INTEGER + 1),
            Err(ContractError::IntegerRange)
        );
        for invalid in ["-1", "1.5", "9007199254740992", "\"1\""] {
            assert!(serde_json::from_str::<BrowserUint53>(invalid).is_err());
        }
        assert!(serde_json::from_str::<BrowserCounter>("0").is_err());
        assert!(serde_json::from_str::<ByteOffset>("9223372036854775808").is_err());
        assert!(serde_json::from_str::<ProtocolVersion>("2").is_err());
        Ok(())
    }

    #[test]
    fn utc_millisecond_timestamps_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let time = WireTimestamp::try_from("2026-10-09T12:34:56.123Z".to_owned())?;
        assert_eq!(WireTimestamp::from_millis(time.millis())?, time);
        assert_eq!(
            serde_json::to_string(&time)?,
            "\"2026-10-09T12:34:56.123Z\""
        );
        for invalid in [
            "2026-02-30T00:00:00Z",
            "2026-10-09T00:00:00+02:00",
            "2026-10-09T00:00:00.000001Z",
            "1791544662185",
        ] {
            assert!(WireTimestamp::try_from(invalid.to_owned()).is_err());
        }
        Ok(())
    }

    #[test]
    fn rejects_traversal_and_oversized_json_before_parse() -> Result<(), ContractError> {
        for invalid in [
            "",
            "/etc/passwd",
            "../file",
            "work/./file",
            "work//file",
            "C:/file",
            "work\\file",
            "work/\0file",
        ] {
            assert!(SandboxPath::try_from(invalid.to_owned()).is_err());
        }
        assert_eq!(
            SandboxPath::try_from("work/result.txt".to_owned())?.as_str(),
            "work/result.txt"
        );
        assert_eq!(
            parse_json::<Vec<u8>>(b"[0,1]", 4),
            Err(ContractError::PayloadTooLarge)
        );
        assert_eq!(parse_json::<Vec<u8>>(b"[0,1]", 5)?, vec![0, 1]);
        assert_eq!(
            parse_json::<Vec<u8>>(b"[0,1]", MAX_JSON_BYTES + 1),
            Err(ContractError::PayloadTooLarge)
        );
        for invalid in ["", "key\n", " key", "key\t", "🔑"] {
            assert!(IdempotencyKey::parse(invalid).is_err());
        }
        assert!(IdempotencyKey::parse("x".repeat(129)).is_err());
        assert_eq!(IdempotencyKey::parse("repeat-1")?.as_str(), "repeat-1");
        Ok(())
    }
}
