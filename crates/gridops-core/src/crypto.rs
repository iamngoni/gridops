use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use anyhow::{Context, Result};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use hmac::{Hmac, Mac};
use secrecy::ExposeSecret;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::Config;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct Vault {
    encryption_key: [u8; 32],
    session_secret: Vec<u8>,
}

impl Vault {
    pub fn from_config(config: &Config) -> Result<Self> {
        let encoded = config
            .encryption_key()
            .context("GRIDOPS_ENCRYPTION_KEY is required")?
            .expose_secret();
        let decoded = STANDARD
            .decode(encoded)
            .context("GRIDOPS_ENCRYPTION_KEY must be base64")?;
        let encryption_key: [u8; 32] = decoded
            .try_into()
            .map_err(|_| anyhow::anyhow!("GRIDOPS_ENCRYPTION_KEY must contain 32 bytes"))?;
        let session_secret = config
            .session_secret()
            .context("GRIDOPS_SESSION_SECRET is required")?
            .expose_secret()
            .as_bytes()
            .to_vec();
        Ok(Self {
            encryption_key,
            session_secret,
        })
    }

    pub fn seal(&self, plaintext: &str) -> Result<String> {
        let cipher =
            Aes256Gcm::new_from_slice(&self.encryption_key).context("invalid encryption key")?;
        let nonce = Nonce::from(rand::random::<[u8; 12]>());
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| anyhow::anyhow!("credential encryption failed"))?;
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(nonce),
            URL_SAFE_NO_PAD.encode(ciphertext)
        ))
    }

    pub fn open(&self, envelope: &str) -> Result<String> {
        let (encoded_nonce, encoded_ciphertext) = envelope
            .split_once('.')
            .context("invalid encrypted value")?;
        let nonce: [u8; 12] = URL_SAFE_NO_PAD
            .decode(encoded_nonce)
            .context("invalid encrypted nonce")?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid encrypted nonce length"))?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(encoded_ciphertext)
            .context("invalid ciphertext")?;
        let cipher =
            Aes256Gcm::new_from_slice(&self.encryption_key).context("invalid encryption key")?;
        let plaintext = cipher
            .decrypt(&Nonce::from(nonce), ciphertext.as_ref())
            .map_err(|_| anyhow::anyhow!("credential authentication failed"))?;
        String::from_utf8(plaintext).context("credential is not UTF-8")
    }

    pub fn sign(&self, value: &str) -> Result<String> {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.session_secret)
            .context("invalid session secret")?;
        mac.update(value.as_bytes());
        Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
    }

    pub fn verify(&self, value: &str, signature: &str) -> bool {
        let Ok(expected) = self.sign(value) else {
            return false;
        };
        expected.as_bytes().ct_eq(signature.as_bytes()).into()
    }
}

pub fn hash_token(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> Vault {
        Vault {
            encryption_key: [7; 32],
            session_secret: b"test-session-secret".to_vec(),
        }
    }

    #[test]
    fn sealed_values_round_trip_and_detect_tampering() -> Result<()> {
        let vault = vault();
        let sealed = vault.seal("github-token")?;
        assert!(!sealed.contains("github-token"));
        assert_eq!(vault.open(&sealed)?, "github-token");
        let mut tampered = sealed;
        tampered.push('A');
        assert!(vault.open(&tampered).is_err());
        Ok(())
    }

    #[test]
    fn signatures_are_bound_to_the_value() -> Result<()> {
        let vault = vault();
        let signature = vault.sign("session")?;
        assert!(vault.verify("session", &signature));
        assert!(!vault.verify("different", &signature));
        Ok(())
    }
}
