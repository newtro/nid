use anyhow::Result;
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum ShadowCmd {
    Enable,
    Disable,
    Commit,
}

pub async fn run(sub: ShadowCmd) -> Result<()> {
    // Shadow-mode toggling writes to config.toml and re-applies hooks.
    // Phase 8 wires the real flip; Phase 1 records intent.
    let paths = crate::cmd::paths::resolve()?;
    paths.ensure()?;
    let intent_path = paths.config_dir.join("shadow_intent");
    match sub {
        ShadowCmd::Enable => {
            std::fs::write(&intent_path, "enable")?;
            println!("shadow mode: enable requested ({})", intent_path.display());
        }
        ShadowCmd::Disable => {
            std::fs::write(&intent_path, "disable")?;
            println!("shadow mode: disable requested");
        }
        ShadowCmd::Commit => {
            std::fs::write(&intent_path, "commit")?;
            println!("shadow mode: commit requested");
        }
    }
    Ok(())
}
