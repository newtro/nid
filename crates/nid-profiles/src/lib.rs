//! Bundled Layer 3 profiles (plan §13).
//!
//! Compiled into the binary from `/profiles/**/*.toml`. Zero runtime
//! filesystem dependency — a fresh install has immediate gain on day 1.

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

use nid_dsl::ast::Profile;

/// Load every bundled profile, parsed. Panics if any shipped profile fails to
/// parse — this is a release-gate invariant (see tests).
pub fn load_all() -> Vec<(String, Profile)> {
    BUNDLED
        .iter()
        .map(|(name, body)| {
            let p = Profile::from_toml(body)
                .unwrap_or_else(|e| panic!("bundled profile {} failed to parse: {}", name, e));
            (name.to_string(), p)
        })
        .collect()
}

/// All bundled fingerprints (useful for `nid profiles list`).
pub fn fingerprints() -> Vec<String> {
    load_all().into_iter().map(|(_, p)| p.meta.fingerprint).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nid_dsl::validator;

    #[test]
    fn every_bundled_profile_parses_and_validates() {
        for (name, p) in load_all() {
            validator::validate_profile(&p).unwrap_or_else(|e| {
                panic!("{} failed validation: {e}", name);
            });
        }
    }
}
