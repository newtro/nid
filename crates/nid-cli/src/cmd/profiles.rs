//! `nid profiles …`

use anyhow::Result;
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum ProfilesCmd {
    /// List profiles (bundled + persisted).
    List,
    /// Inspect a single profile by fingerprint.
    Inspect { fingerprint: String },
    /// Pin a profile version.
    Pin { fingerprint: String },
    /// Revoke (force re-synthesis path) a profile.
    Revoke { fingerprint: String },
    /// Export a profile as a signed tarball.
    Export { fingerprint: String, output: std::path::PathBuf },
    /// Import a signed profile tarball.
    Import { path: std::path::PathBuf },
    /// Re-sign an exported tarball with an org key.
    Sign {
        tarball: std::path::PathBuf,
        #[arg(long)]
        key: std::path::PathBuf,
    },
    /// Roll back to the most recent superseded version.
    Rollback { fingerprint: String },
    /// Permanently remove a profile.
    Purge { fingerprint: String },
}

pub async fn run(sub: ProfilesCmd) -> Result<()> {
    match sub {
        ProfilesCmd::List => {
            let bundled = nid_profiles::load_all();
            println!("Bundled profiles ({}):", bundled.len());
            for (_name, p) in &bundled {
                println!(
                    "  {} v{} [{}]",
                    p.meta.fingerprint,
                    p.meta.version,
                    p.meta
                        .format_claim
                        .as_ref()
                        .map(|_| "format-claim")
                        .unwrap_or("plain")
                );
            }
            Ok(())
        }
        ProfilesCmd::Inspect { fingerprint } => {
            let all = nid_profiles::load_all();
            let match_ = all.iter().find(|(_, p)| p.meta.fingerprint == fingerprint);
            match match_ {
                Some((_, p)) => {
                    let toml = p.to_toml()?;
                    println!("{toml}");
                    Ok(())
                }
                None => {
                    println!("no profile matching fingerprint `{fingerprint}`");
                    Ok(())
                }
            }
        }
        other => {
            println!("subcommand {:?} wired in a later phase", other);
            Ok(())
        }
    }
}
