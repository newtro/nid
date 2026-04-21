//! Generate a new ed25519 release signing keypair. Prints the hex-encoded
//! 32-byte seed on stdout (feed into NID_RELEASE_SIGNING_KEY secret) and the
//! corresponding pubkey + key_id on stderr.

use ed25519_dalek::SigningKey;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::io::Write;

fn main() {
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let sk = SigningKey::from_bytes(&seed);
    let pk = sk.verifying_key();
    let mut h = Sha256::new();
    h.update(pk.as_bytes());
    let key_id = hex::encode(&h.finalize()[..8]);

    // Seed on stdout so `>` redirect gets just the secret.
    println!("{}", hex::encode(seed));
    // Metadata on stderr.
    writeln!(
        std::io::stderr(),
        "# key_id:  {key_id}\n# pubkey:  {}",
        hex::encode(pk.as_bytes())
    )
    .ok();
}
