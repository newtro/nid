//! `.nidprofile` tarball format — signed, portable profile artifact
//! (plan §6.7, §11.5).
//!
//! The tarball (uncompressed — entries are already compressed) contains:
//!
//!   - `profile.toml`      : profile body as TOML
//!   - `manifest.json`     : `{ "signer_key_id": "...", "signed_at": unix_ts,
//!                             "sha256": "<sha of profile.toml bytes>" }`
//!   - `signature.bin`     : ed25519 signature of `manifest.json` bytes
//!   - `signer.pub`        : 32-byte raw pubkey bytes
//!
//! On import, the verifier:
//!   1. reads signer.pub, recomputes key_id, and looks it up in the trust
//!      keyring (refusing if missing).
//!   2. reads signature.bin and manifest.json and verifies the signature.
//!   3. checks manifest.sha256 matches sha256(profile.toml).
//!   4. validates the profile via DSL validator.

use crate::ast::Profile;
use crate::validator;
use ed25519_dalek::{SigningKey, VerifyingKey};
use nid_core::signing;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use thiserror::Error;

pub const PROFILE_ENTRY: &str = "profile.toml";
pub const MANIFEST_ENTRY: &str = "manifest.json";
pub const SIGNATURE_ENTRY: &str = "signature.bin";
pub const SIGNER_PUB_ENTRY: &str = "signer.pub";

#[derive(Debug, Error)]
pub enum NidProfileError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("tar: {0}")]
    Tar(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml parse: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("toml serialize: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("dsl validation: {0}")]
    Validate(#[from] validator::ValidationError),
    #[error("signing: {0}")]
    Signing(#[from] signing::SigningError),
    #[error("manifest SHA mismatch: profile body has been tampered with")]
    BodyTampered,
    #[error("missing entry in tarball: {0}")]
    MissingEntry(&'static str),
    #[error("signer key `{0}` is not trusted (use `nid trust add` to add it)")]
    UntrustedSigner(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub signer_key_id: String,
    pub signed_at: i64,
    pub sha256: String,
    /// Profile schema version (forward-compat marker).
    pub schema: String,
}

/// Pack a profile into a signed tarball. Writes to `out` (usually a file).
pub fn pack<W: std::io::Write>(
    out: W,
    profile: &Profile,
    signer: &SigningKey,
) -> Result<(), NidProfileError> {
    validator::validate_profile(profile)?;

    let profile_toml = profile.to_toml()?;
    let body_bytes = profile_toml.as_bytes();
    let mut h = Sha256::new();
    h.update(body_bytes);
    let body_sha = hex::encode(h.finalize());

    let manifest = Manifest {
        signer_key_id: signing::key_id(&signer.verifying_key()),
        signed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        sha256: body_sha,
        schema: profile.meta.schema.clone(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let signature = signing::sign(signer, &manifest_bytes);
    let pub_bytes = signer.verifying_key().as_bytes().to_vec();

    let mut builder = tar::Builder::new(out);
    append_entry(&mut builder, PROFILE_ENTRY, body_bytes)?;
    append_entry(&mut builder, MANIFEST_ENTRY, &manifest_bytes)?;
    append_entry(&mut builder, SIGNATURE_ENTRY, &signature)?;
    append_entry(&mut builder, SIGNER_PUB_ENTRY, &pub_bytes)?;
    builder.finish()?;
    Ok(())
}

fn append_entry<W: std::io::Write>(
    b: &mut tar::Builder<W>,
    name: &str,
    bytes: &[u8],
) -> Result<(), NidProfileError> {
    let mut header = tar::Header::new_gnu();
    header
        .set_path(name)
        .map_err(|e| NidProfileError::Tar(e.to_string()))?;
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    b.append(&header, bytes)
        .map_err(|e| NidProfileError::Tar(e.to_string()))?;
    Ok(())
}

/// Unpacked + verified contents of a `.nidprofile` tarball.
#[derive(Debug, Clone)]
pub struct UnpackedProfile {
    pub profile: Profile,
    pub manifest: Manifest,
    pub signer_pubkey: VerifyingKey,
}

/// `trusted_keys` is the set of key IDs that may sign a profile. Pass an
/// empty slice to refuse all imports (equivalent to T4 refusal in plan §11.2).
pub fn unpack_and_verify(
    bytes: &[u8],
    trusted_key_ids: &[String],
) -> Result<UnpackedProfile, NidProfileError> {
    let mut profile_bytes: Option<Vec<u8>> = None;
    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut signature_bytes: Option<Vec<u8>> = None;
    let mut pubkey_bytes: Option<Vec<u8>> = None;

    let mut ar = tar::Archive::new(Cursor::new(bytes));
    for entry in ar
        .entries()
        .map_err(|e| NidProfileError::Tar(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| NidProfileError::Tar(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NidProfileError::Tar(e.to_string()))?
            .to_string_lossy()
            .to_string();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut buf)?;
        match path.as_str() {
            PROFILE_ENTRY => profile_bytes = Some(buf),
            MANIFEST_ENTRY => manifest_bytes = Some(buf),
            SIGNATURE_ENTRY => signature_bytes = Some(buf),
            SIGNER_PUB_ENTRY => pubkey_bytes = Some(buf),
            _ => {}
        }
    }
    let profile_bytes = profile_bytes.ok_or(NidProfileError::MissingEntry(PROFILE_ENTRY))?;
    let manifest_bytes = manifest_bytes.ok_or(NidProfileError::MissingEntry(MANIFEST_ENTRY))?;
    let signature_bytes = signature_bytes.ok_or(NidProfileError::MissingEntry(SIGNATURE_ENTRY))?;
    let pubkey_bytes = pubkey_bytes.ok_or(NidProfileError::MissingEntry(SIGNER_PUB_ENTRY))?;

    let pubkey = signing::pubkey_from_bytes(&pubkey_bytes)?;
    let signer_id = signing::key_id(&pubkey);

    if !trusted_key_ids.iter().any(|k| k == &signer_id) {
        return Err(NidProfileError::UntrustedSigner(signer_id));
    }

    // Verify signature over manifest bytes.
    signing::verify(&pubkey, &manifest_bytes, &signature_bytes)?;

    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)?;

    // Manifest signer must match the embedded pubkey — prevents swapped pubkey.
    if manifest.signer_key_id != signer_id {
        return Err(NidProfileError::UntrustedSigner(signer_id));
    }

    // Body integrity check.
    let mut h = Sha256::new();
    h.update(&profile_bytes);
    let got = hex::encode(h.finalize());
    if got != manifest.sha256 {
        return Err(NidProfileError::BodyTampered);
    }

    let profile = Profile::from_toml(std::str::from_utf8(&profile_bytes).unwrap_or(""))?;
    validator::validate_profile(&profile)?;

    Ok(UnpackedProfile {
        profile,
        manifest,
        signer_pubkey: pubkey,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{FormatClaim, Invariant, InvariantCheck, Meta, Rule, RuleKind};

    fn sample_profile() -> Profile {
        Profile {
            meta: Meta {
                fingerprint: "test profile".into(),
                version: "1.0.0".into(),
                schema: "1.0".into(),
                format_claim: Some(FormatClaim::Plain),
                description: Some("test".into()),
            },
            rules: vec![Rule {
                kind: RuleKind::Dedup,
            }],
            invariants: vec![Invariant {
                name: "Exit".into(),
                check: InvariantCheck::ExitLinePreserved,
            }],
            self_tests: vec![],
        }
    }

    #[test]
    fn pack_unpack_roundtrip_with_trusted_signer() {
        let sk = signing::generate_keypair();
        let signer_id = signing::key_id(&sk.verifying_key());
        let profile = sample_profile();

        let mut buf = Vec::new();
        pack(&mut buf, &profile, &sk).unwrap();

        let unpacked = unpack_and_verify(&buf, &[signer_id]).unwrap();
        assert_eq!(unpacked.profile.meta.fingerprint, "test profile");
    }

    #[test]
    fn unpack_refuses_untrusted_signer() {
        let sk = signing::generate_keypair();
        let profile = sample_profile();

        let mut buf = Vec::new();
        pack(&mut buf, &profile, &sk).unwrap();

        let r = unpack_and_verify(&buf, &[]);
        assert!(matches!(r, Err(NidProfileError::UntrustedSigner(_))));
    }

    #[test]
    fn unpack_detects_body_tamper() {
        let sk = signing::generate_keypair();
        let signer_id = signing::key_id(&sk.verifying_key());
        let profile = sample_profile();
        let mut buf = Vec::new();
        pack(&mut buf, &profile, &sk).unwrap();

        // Corrupt the profile.toml entry inside the tarball by rebuilding it
        // with a different body but same manifest.
        let mut corrupted = Vec::new();
        {
            let mut b = tar::Builder::new(&mut corrupted);
            // Read entries but swap profile body.
            let mut ar = tar::Archive::new(Cursor::new(&buf));
            for entry in ar.entries().unwrap() {
                let mut e = entry.unwrap();
                let path = e.path().unwrap().to_string_lossy().to_string();
                let mut body = Vec::new();
                std::io::Read::read_to_end(&mut e, &mut body).unwrap();
                let new_body = if path == PROFILE_ENTRY {
                    b"tampered\n".to_vec()
                } else {
                    body
                };
                let mut header = tar::Header::new_gnu();
                header.set_path(&path).unwrap();
                header.set_size(new_body.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                b.append(&header, new_body.as_slice()).unwrap();
            }
            b.finish().unwrap();
        }

        let r = unpack_and_verify(&corrupted, &[signer_id]);
        assert!(
            matches!(r, Err(NidProfileError::BodyTampered)),
            "got {:?}",
            r
        );
    }
}
