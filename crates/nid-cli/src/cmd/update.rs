use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct UpdateArgs {
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub to: Option<String>,
    #[arg(long, default_value = "stable")]
    pub channel: String,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub from: Option<std::path::PathBuf>,
}

pub async fn run(args: UpdateArgs) -> Result<()> {
    println!(
        "update: check={} to={:?} channel={} dry_run={} from={:?}",
        args.check, args.to, args.channel, args.dry_run, args.from
    );
    println!("(signed update flow wired in Phase 8)");
    Ok(())
}
