//! Password encryption for the secrets in an exported configuration: PBKDF2-HMAC-SHA256 turns the
//! password into a key, AES-256-GCM seals the bytes. Everything but the key travels with the file,
//! so the same password opens it on any computer.

use anyhow::{anyhow, Result};
use base64::Engine as _;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

/// OWASP's 2023 figure for PBKDF2-HMAC-SHA256.
pub const ITERATIONS: u32 = 600_000;
const SALT_LEN: usize = 16;

/// Sealed bytes and how to open them with the password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sealed {
    /// `pbkdf2-sha256+aes-256-gcm`.
    pub scheme: String,
    pub iterations: u32,
    pub salt: String,
    pub nonce: String,
    /// Ciphertext followed by the tag, base64.
    pub data: String,
}

const SCHEME: &str = "pbkdf2-sha256+aes-256-gcm";

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn derive(password: &str, salt: &[u8], iterations: u32) -> Result<LessSafeKey> {
    let iterations = NonZeroU32::new(iterations).ok_or_else(|| anyhow!("bad iteration count"))?;
    let mut key = [0u8; 32];
    ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, iterations, salt, password.as_bytes(), &mut key);
    let unbound = UnboundKey::new(&AES_256_GCM, &key).map_err(|_| anyhow!("bad key"))?;
    Ok(LessSafeKey::new(unbound))
}

pub fn seal(password: &str, plain: &[u8]) -> Result<Sealed> {
    seal_with(password, plain, ITERATIONS)
}

fn seal_with(password: &str, plain: &[u8], iterations: u32) -> Result<Sealed> {
    let rng = SystemRandom::new();
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    rng.fill(&mut salt).map_err(|_| anyhow!("no randomness"))?;
    rng.fill(&mut nonce).map_err(|_| anyhow!("no randomness"))?;
    let key = derive(password, &salt, iterations)?;
    let mut data = plain.to_vec();
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(SCHEME.as_bytes()), &mut data)
        .map_err(|_| anyhow!("encryption failed"))?;
    Ok(Sealed { scheme: SCHEME.into(), iterations, salt: b64().encode(salt), nonce: b64().encode(nonce), data: b64().encode(data) })
}

/// Why sealed bytes did not open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Wrong password (or the file was changed).
    WrongPassword,
    Malformed,
}

pub fn open(password: &str, sealed: &Sealed) -> Result<Vec<u8>, OpenError> {
    if sealed.scheme != SCHEME || sealed.iterations == 0 || sealed.iterations > 10_000_000 {
        return Err(OpenError::Malformed);
    }
    let salt = b64().decode(&sealed.salt).map_err(|_| OpenError::Malformed)?;
    let nonce: [u8; NONCE_LEN] = b64().decode(&sealed.nonce).ok().and_then(|n| n.try_into().ok()).ok_or(OpenError::Malformed)?;
    let mut data = b64().decode(&sealed.data).map_err(|_| OpenError::Malformed)?;
    let key = derive(password, &salt, sealed.iterations).map_err(|_| OpenError::Malformed)?;
    let plain = key
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(SCHEME.as_bytes()), &mut data)
        .map_err(|_| OpenError::WrongPassword)?;
    Ok(plain.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_with_the_same_password_only() {
        let sealed = seal_with("correct horse", b"placeholder secret", 1_000).unwrap();
        assert_eq!(open("correct horse", &sealed).unwrap(), b"placeholder secret");
        assert_eq!(open("wrong horse", &sealed), Err(OpenError::WrongPassword));
    }

    #[test]
    fn a_changed_file_does_not_open() {
        let mut sealed = seal_with("pw", b"example value", 1_000).unwrap();
        let mut bytes = b64().decode(&sealed.data).unwrap();
        bytes[0] ^= 1;
        sealed.data = b64().encode(bytes);
        assert_eq!(open("pw", &sealed), Err(OpenError::WrongPassword));
        sealed.scheme = "rot13".into();
        assert_eq!(open("pw", &sealed), Err(OpenError::Malformed));
    }

    #[test]
    fn every_seal_is_different() {
        let a = seal_with("pw", b"x", 1_000).unwrap();
        let b = seal_with("pw", b"x", 1_000).unwrap();
        assert_ne!(a.salt, b.salt);
        assert_ne!(a.data, b.data);
    }
}
