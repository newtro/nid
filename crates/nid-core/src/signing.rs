//! Ed25519 signing primitives (plan §10.3 release signing, §11.2 profile trust,
//! §11.5 org profile sharing).
//!
//! Wraps `ed25519-dalek` with a tiny wire format:
//!
//!   ```text
//!   KeyId       = hex(sha256(public_key_bytes))[..16]    // 8 bytes
//!   Signature   = ed25519-dalek raw 64-byte signature
//!   RotationRecord = { from_key_id, to_key_bytes, signature_by_from }
//!     where signature_by_from = ed25519(from_key, "nid-rotate:" + to_key_bytes)
//!   ```
//!
//! The binary ships with a trust anchor (`release_key_pub`); when a release
//! artifact arrives signed by an unknown key, nid walks the shipped rotation
//! record chain from its anchor forward until it finds the signing key (or
//! refuses the update). No out-of-band trust.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const KEY_ID_LEN: usize = 16; // hex chars
const ROTATION_PREFIX: &[u8] = b"nid-rotate:";

#[derive(Debug, Error)]
pub enum SigningError {
    #[error("bad key bytes")]
    BadKey,
    #[error("bad signature bytes")]
    BadSig,
    #[error("signature verification failed")]
    VerifyFailed,
    #[error("rotation chain: cannot walk from {from} to {target}")]
    NoRotationPath { from: String, target: String },
    #[error("rotation record self-signature invalid")]
    BadRotationSig,
}

/// Hex-encoded short identifier for a public key — first 8 bytes of
/// SHA-256(pubkey_bytes), rendered as 16 hex chars.
pub fn key_id(pk: &VerifyingKey) -> String {
    let mut h = Sha256::new();
    h.update(pk.as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..8])
}

/// Generate a new random Ed25519 keypair.
pub fn generate_keypair() -> SigningKey {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    SigningKey::from_bytes(&bytes)
}

/// Sign arbitrary bytes.
pub fn sign(sk: &SigningKey, msg: &[u8]) -> Vec<u8> {
    sk.sign(msg).to_bytes().to_vec()
}

/// Verify a raw 64-byte signature.
pub fn verify(pk: &VerifyingKey, msg: &[u8], sig: &[u8]) -> Result<(), SigningError> {
    let s = Signature::from_slice(sig).map_err(|_| SigningError::BadSig)?;
    pk.verify(msg, &s).map_err(|_| SigningError::VerifyFailed)
}

/// Load a public key from its 32-byte encoding.
pub fn pubkey_from_bytes(bytes: &[u8]) -> Result<VerifyingKey, SigningError> {
    let arr: [u8; 32] = bytes.try_into().map_err(|_| SigningError::BadKey)?;
    VerifyingKey::from_bytes(&arr).map_err(|_| SigningError::BadKey)
}

/// A single rotation link: the `from` key attests that the `to` key is
/// its successor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationLink {
    pub from_key_id: String,
    pub to_key_id: String,
    /// 32-byte verifying-key bytes of the successor, hex-encoded.
    pub to_key_hex: String,
    /// Signature of `ROTATION_PREFIX || to_key_bytes` under the `from` key.
    pub signature_hex: String,
}

/// A sequence of rotation links forming a chain.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RotationChain {
    pub links: Vec<RotationLink>,
}

impl RotationChain {
    /// Produce a new link: `from` attests `to` is its successor.
    pub fn new_link(from_sk: &SigningKey, to_pk: &VerifyingKey) -> RotationLink {
        let from_pk = from_sk.verifying_key();
        let msg: Vec<u8> = ROTATION_PREFIX
            .iter()
            .copied()
            .chain(to_pk.as_bytes().iter().copied())
            .collect();
        let sig = from_sk.sign(&msg);
        RotationLink {
            from_key_id: key_id(&from_pk),
            to_key_id: key_id(to_pk),
            to_key_hex: hex::encode(to_pk.as_bytes()),
            signature_hex: hex::encode(sig.to_bytes()),
        }
    }

    /// Walk the chain starting at `anchor`; return the verified pubkey for
    /// `target_key_id`, or `Err` if no valid path exists.
    pub fn resolve(
        &self,
        anchor: &VerifyingKey,
        target_key_id: &str,
    ) -> Result<VerifyingKey, SigningError> {
        let mut cur_id = key_id(anchor);
        let mut cur_pk = *anchor;
        if cur_id == target_key_id {
            return Ok(cur_pk);
        }
        let mut hops = 0usize;
        loop {
            if hops > 64 {
                return Err(SigningError::NoRotationPath {
                    from: key_id(anchor),
                    target: target_key_id.to_string(),
                });
            }
            let Some(link) = self.links.iter().find(|l| l.from_key_id == cur_id) else {
                return Err(SigningError::NoRotationPath {
                    from: cur_id,
                    target: target_key_id.to_string(),
                });
            };
            let to_bytes = hex::decode(&link.to_key_hex).map_err(|_| SigningError::BadKey)?;
            let next_pk = pubkey_from_bytes(&to_bytes)?;
            let next_id = key_id(&next_pk);
            if next_id != link.to_key_id {
                return Err(SigningError::BadRotationSig);
            }
            let sig_bytes = hex::decode(&link.signature_hex).map_err(|_| SigningError::BadSig)?;
            let mut msg = ROTATION_PREFIX.to_vec();
            msg.extend_from_slice(&to_bytes);
            verify(&cur_pk, &msg, &sig_bytes)?;
            if next_id == target_key_id {
                return Ok(next_pk);
            }
            cur_pk = next_pk;
            cur_id = next_id;
            hops += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let sk = generate_keypair();
        let sig = sign(&sk, b"hello");
        verify(&sk.verifying_key(), b"hello", &sig).unwrap();
        // Wrong message.
        assert!(verify(&sk.verifying_key(), b"bye", &sig).is_err());
    }

    #[test]
    fn key_id_is_deterministic() {
        let sk = generate_keypair();
        let pk = sk.verifying_key();
        assert_eq!(key_id(&pk), key_id(&pk));
        assert_eq!(key_id(&pk).len(), KEY_ID_LEN);
    }

    #[test]
    fn rotation_chain_resolves() {
        // Build: A → B → C; verify starting from A we can reach C.
        let a = generate_keypair();
        let b = generate_keypair();
        let c = generate_keypair();

        let l1 = RotationChain::new_link(&a, &b.verifying_key());
        let l2 = RotationChain::new_link(&b, &c.verifying_key());
        let chain = RotationChain {
            links: vec![l1, l2],
        };

        let resolved = chain
            .resolve(&a.verifying_key(), &key_id(&c.verifying_key()))
            .unwrap();
        assert_eq!(key_id(&resolved), key_id(&c.verifying_key()));
    }

    #[test]
    fn rotation_chain_refuses_missing_link() {
        let a = generate_keypair();
        let b = generate_keypair();
        let c = generate_keypair();
        // Only link A → B.
        let chain = RotationChain {
            links: vec![RotationChain::new_link(&a, &b.verifying_key())],
        };
        let r = chain.resolve(&a.verifying_key(), &key_id(&c.verifying_key()));
        assert!(matches!(r, Err(SigningError::NoRotationPath { .. })));
    }

    #[test]
    fn rotation_chain_refuses_tampered_link() {
        let a = generate_keypair();
        let b = generate_keypair();
        let c = generate_keypair();
        // Build a link from A → B; then rewrite its to_key_hex to C's, keeping
        // A's signature (which was over B). Signature must fail.
        let mut l = RotationChain::new_link(&a, &b.verifying_key());
        l.to_key_hex = hex::encode(c.verifying_key().as_bytes());
        l.to_key_id = key_id(&c.verifying_key());
        let chain = RotationChain { links: vec![l] };
        let r = chain.resolve(&a.verifying_key(), &key_id(&c.verifying_key()));
        assert!(matches!(r, Err(SigningError::VerifyFailed)));
    }

    #[test]
    fn anchor_equals_target_resolves_immediately() {
        let a = generate_keypair();
        let chain = RotationChain::default();
        let r = chain
            .resolve(&a.verifying_key(), &key_id(&a.verifying_key()))
            .unwrap();
        assert_eq!(key_id(&r), key_id(&a.verifying_key()));
    }
}
