//! AES-GCM sealed blobs for unredacted raw output (plan §11.1).
//!
//! Serialized layout (inside the content-addressed blob store):
//!
//!   version(1) || nonce(12) || ciphertext
//!
//! Key derivation: the local key file at `<data>/key` is a raw 32-byte
//! key. If the file is missing, `load_or_create` generates one (0600 perms
//! on unix) — this is the user's machine-local secret and must NOT leave.
//!
//! On `get_sealed`, the blob hash is the hash of the ciphertext (not the
//! plaintext) so Blob roundtrip verification still works.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use std::io::Read;
use std::path::Path;
use thiserror::Error;

const VERSION_V1: u8 = 1;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

#[derive(Debug, Error)]
pub enum SealError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("aes-gcm: {0}")]
    Crypto(String),
    #[error("bad sealed blob: {0}")]
    Format(&'static str),
}

/// Load the machine-local 32-byte key from disk, creating one if missing.
/// Tightens perms to 0600 on unix.
pub fn load_or_create_key(path: &Path) -> Result<[u8; KEY_LEN], SealError> {
    if let Ok(bytes) = std::fs::read(path) {
        if bytes.len() == KEY_LEN {
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(&bytes);
            return Ok(key);
        }
    }
    let mut key = [0u8; KEY_LEN];
    rand::thread_rng().fill_bytes(&mut key);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Write atomic.
    let tmp = path.with_extension("keytmp");
    std::fs::write(&tmp, key)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(key)
}

/// Encrypt `plaintext` with the local key. Returns a versioned, self-describing
/// blob suitable for content-addressed storage.
pub fn seal(plaintext: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>, SealError> {
    let cipher = Aes256Gcm::new(key.into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| SealError::Crypto(e.to_string()))?;
    let mut out = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
    out.push(VERSION_V1);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Inverse of `seal`.
pub fn open(sealed: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>, SealError> {
    if sealed.is_empty() {
        return Err(SealError::Format("empty"));
    }
    match sealed[0] {
        VERSION_V1 => {
            if sealed.len() < 1 + NONCE_LEN {
                return Err(SealError::Format("too short"));
            }
            let cipher = Aes256Gcm::new(key.into());
            let nonce = Nonce::from_slice(&sealed[1..1 + NONCE_LEN]);
            let ct = &sealed[1 + NONCE_LEN..];
            cipher
                .decrypt(nonce, ct)
                .map_err(|e| SealError::Crypto(e.to_string()))
        }
        v => Err(SealError::Format(Box::leak(
            format!("unknown version {v}").into_boxed_str(),
        ))),
    }
}

/// Small helper for callers that don't want to keep the key around.
pub fn open_reader<R: Read>(mut r: R, key: &[u8; KEY_LEN]) -> Result<Vec<u8>, SealError> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    open(&buf, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn seal_open_roundtrip() {
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        let plaintext = b"token: ghp_1234567890abcdefghij1234567890abcdefghij\n";
        let sealed = seal(plaintext, &key).unwrap();
        let opened = open(&sealed, &key).unwrap();
        assert_eq!(&opened, plaintext);
    }

    #[test]
    fn open_wrong_key_fails() {
        let mut k1 = [0u8; 32];
        let mut k2 = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut k1);
        rand::thread_rng().fill_bytes(&mut k2);
        let sealed = seal(b"x", &k1).unwrap();
        let res = open(&sealed, &k2);
        assert!(matches!(res, Err(SealError::Crypto(_))));
    }

    #[test]
    fn load_or_create_key_persists_across_calls() {
        let t = TempDir::new().unwrap();
        let path = t.path().join("key");
        let k1 = load_or_create_key(&path).unwrap();
        let k2 = load_or_create_key(&path).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        let plaintext = b"hello";
        let mut sealed = seal(plaintext, &key).unwrap();
        // Flip a byte in the ciphertext.
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert!(open(&sealed, &key).is_err());
    }
}
