//! Verifier-only enrollment and host credential values.
//!
//! Plaintext values are generated in memory for one dedicated receipt and are
//! never serialized by this module. `SQLite` receives only a SHA-256
//! base64url verifier; all secret-bearing `Debug` implementations are
//! deliberately redacted.

use std::fmt;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use secrecy::{ExposeSecret as _, SecretString};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

const SECRET_BYTES: usize = 32;
const SECRET_TEXT_LENGTH: usize = 43;

/// A one-time plaintext returned only by an enrollment or rotation receipt.
#[derive(Clone)]
pub struct OneTimeSecret(SecretString);

impl OneTimeSecret {
    pub(crate) fn generate() -> Self {
        let bytes = rand::random::<[u8; SECRET_BYTES]>();
        Self(SecretString::from(URL_SAFE_NO_PAD.encode(bytes)))
    }

    pub(crate) fn parse(value: &str) -> Result<Self, CredentialError> {
        validate_secret_text(value)?;
        Ok(Self(SecretString::from(value.to_owned())))
    }

    /// Expose the plaintext only at the dedicated receipt boundary.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }

    pub(crate) fn verifier(&self) -> Verifier {
        Verifier::from_secret(self.expose())
    }
}

impl fmt::Debug for OneTimeSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OneTimeSecret(REDACTED)")
    }
}

/// A SHA-256 verifier stored in the registry.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Verifier(String);

impl Verifier {
    pub(crate) fn from_secret(secret: &str) -> Self {
        Self(URL_SAFE_NO_PAD.encode(Sha256::digest(secret.as_bytes())))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_hash(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn matches_verifier(&self, other: &Self) -> bool {
        self.0.as_bytes().ct_eq(other.0.as_bytes()).into()
    }
}

pub(crate) fn validate_secret_for_authentication(value: &str) -> Result<(), CredentialError> {
    validate_secret_text(value)
}

impl fmt::Debug for Verifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Verifier(REDACTED)")
    }
}

/// Stable validation failures for host supplied credential material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CredentialError {
    #[error("credential must be exactly 32 random bytes encoded as base64url")]
    InvalidEncoding,
}

fn validate_secret_text(value: &str) -> Result<(), CredentialError> {
    if value.len() != SECRET_TEXT_LENGTH || !value.bytes().all(is_base64url) {
        return Err(CredentialError::InvalidEncoding);
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| CredentialError::InvalidEncoding)?;
    if decoded.len() != SECRET_BYTES {
        return Err(CredentialError::InvalidEncoding);
    }
    if URL_SAFE_NO_PAD.encode(decoded) != value {
        return Err(CredentialError::InvalidEncoding);
    }
    Ok(())
}

const fn is_base64url(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_secret_is_bounded_and_verifies() {
        let secret = OneTimeSecret::generate();
        assert_eq!(secret.expose().len(), SECRET_TEXT_LENGTH);
        let verifier = secret.verifier();
        assert!(verifier.matches_verifier(&Verifier::from_secret(secret.expose())));
        assert!(!verifier.matches_verifier(&Verifier::from_secret(
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        )));
        assert_eq!(format!("{secret:?}"), "OneTimeSecret(REDACTED)");
        assert_eq!(format!("{verifier:?}"), "Verifier(REDACTED)");
    }

    #[test]
    fn malformed_host_secret_is_rejected() {
        for candidate in [
            "",
            "short",
            &format!("{}B", "A".repeat(42)),
            &"!".repeat(43),
        ] {
            assert!(matches!(
                OneTimeSecret::parse(candidate),
                Err(CredentialError::InvalidEncoding)
            ));
        }
    }
}
