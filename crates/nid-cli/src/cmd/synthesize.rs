use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct SynthesizeArgs {
    pub command: Vec<String>,
    #[arg(long)]
    pub force: bool,
}

pub async fn run(args: SynthesizeArgs) -> Result<()> {
    if args.command.is_empty() {
        anyhow::bail!("synthesize requires a command");
    }
    let fp = nid_core::fingerprint(&args.command);
    println!("synthesize requested for fingerprint `{fp}` (force={})", args.force);
    println!("(full wiring in Phase 4; here we record the request)");
    Ok(())
}
