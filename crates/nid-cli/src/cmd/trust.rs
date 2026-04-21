use anyhow::Result;
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum TrustCmd {
    Add {
        key: std::path::PathBuf,
        #[arg(long)]
        label: Option<String>,
    },
    Revoke {
        key_id: String,
    },
    List,
}

pub async fn run(sub: TrustCmd) -> Result<()> {
    match sub {
        TrustCmd::List => println!("(no trusted keys yet — wired in Phase 8)"),
        TrustCmd::Add { key, label } => {
            println!(
                "trust add from {} label={:?} (wired in Phase 8)",
                key.display(),
                label
            );
        }
        TrustCmd::Revoke { key_id } => {
            println!("trust revoke {} (wired in Phase 8)", key_id);
        }
    }
    Ok(())
}
