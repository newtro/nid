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
    #[error(
        "sealed key at {path} is missing or corrupt, but sealed raw blobs exist in the store. \
         Regenerating the key would orphan prior raw output permanently. Recovery options: \
         (a) restore the key file from backup, (b) run `nid gc --retention-days 0` to purge \
         the now-unreadable raw blobs, or (c) delete {path} AND clear sealed-raw blobs manually."
    )]
    KeyMissingButBlobsExist { path: String },
    #[error("sealed key at {path} is the wrong size ({got} bytes, expected {expected})")]
    KeyWrongSize {
        path: String,
        got: usize,
        expected: usize,
    },
}

/// Load the machine-local 32-byte key from disk, creating one if missing AND
/// no sealed blobs exist yet (fresh install path).
///
/// **Safety note**: if the key is missing but the blob store already contains
/// sealed raw blobs, we REFUSE to regenerate — blindly making a new key
/// would orphan every prior raw output silently. The caller must either
/// restore the key from backup or run `nid gc` to purge the orphans.
///
/// `blobs_root` is the directory where sealed blobs live (for the
/// "do prior sealed blobs exist?" check). Pass None for paths that
/// don't need the safety check (e.g. the `nid-package` helper).
pub fn load_or_create_key(path: &Path) -> Result<[u8; KEY_LEN], SealError> {
    load_or_create_key_safe(path, None)
}

/// Same as `load_or_create_key`, but with the sealed-blobs-exist safety
/// check. Pass `Some(blobs_root)` to enable the check.
pub fn load_or_create_key_safe(
    path: &Path,
    blobs_root: Option<&Path>,
) -> Result<[u8; KEY_LEN], SealError> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() == KEY_LEN => {
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(&bytes);
            return Ok(key);
        }
        Ok(bytes) => {
            return Err(SealError::KeyWrongSize {
                path: path.display().to_string(),
                got: bytes.len(),
                expected: KEY_LEN,
            });
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(SealError::Io(e));
        }
        Err(_) => {
            // NotFound — continue to the regenerate path below, but first
            // check whether any sealed blobs already exist.
        }
    }

    // Key is missing. Refuse to regenerate if prior sealed blobs are present.
    if let Some(blobs_dir) = blobs_root {
        if blobs_dir_contains_sealed_blob(blobs_dir) {
            return Err(SealError::KeyMissingButBlobsExist {
                path: path.display().to_string(),
            });
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

/// Cheap probe: does `blobs_root` contain at least one file that looks like
/// a sealed blob (first byte == VERSION_V1)? Used by `load_or_create_key_safe`
/// to refuse silent key regeneration when orphaning would result.
fn blobs_dir_contains_sealed_blob(blobs_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(blobs_root) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Read only the first byte to keep this cheap; sealed blobs are
        // zstd-compressed so the first byte won't be literally 0x01 unless we
        // decompress. Shortcut: a real orphan-check would decompress but for
        // a cheap probe we just require that *any* raw blob exists.
        // Since all raw blobs in the store are sealed (post-upgrade) or
        // legacy plaintext — either way the existence of raw/sample/... blobs
        // means the key loss is destructive. Probe by filename pattern.
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if name.starts_with("sha256-") && name.ends_with(".zst") {
                return true;
            }
        }
    }
    false
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

    #[test]
    fn load_or_create_key_safe_refuses_when_blobs_exist() {
        // Simulate key-loss-with-blobs-present: create a blobs dir with one
        // sha256-*.zst file, then call load_or_create_key_safe on a
        // nonexistent key. It MUST refuse rather than silently regenerate.
        let t = TempDir::new().unwrap();
        let key_path = t.path().join("key");
        let blobs_dir = t.path().join("blobs");
        std::fs::create_dir_all(&blobs_dir).unwrap();
        std::fs::write(blobs_dir.join("sha256-deadbeef.zst"), b"fake").unwrap();

        let res = load_or_create_key_safe(&key_path, Some(&blobs_dir));
        assert!(
            matches!(res, Err(SealError::KeyMissingButBlobsExist { .. })),
            "expected KeyMissingButBlobsExist, got {res:?}"
        );
        // Key file must NOT have been created.
        assert!(!key_path.exists());
    }

    #[test]
    fn load_or_create_key_safe_regenerates_on_empty_blob_store() {
        // Opposite case: empty blobs dir is a fresh install → regenerate.
        let t = TempDir::new().unwrap();
        let key_path = t.path().join("key");
        let blobs_dir = t.path().join("blobs");
        std::fs::create_dir_all(&blobs_dir).unwrap();
        let key = load_or_create_key_safe(&key_path, Some(&blobs_dir)).unwrap();
        assert_eq!(key.len(), 32);
        assert!(key_path.exists());
    }

    #[test]
    fn load_or_create_key_returns_wrong_size_error() {
        let t = TempDir::new().unwrap();
        let key_path = t.path().join("key");
        std::fs::write(&key_path, b"too short").unwrap();
        let res = load_or_create_key(&key_path);
        assert!(matches!(res, Err(SealError::KeyWrongSize { .. })));
    }
}
