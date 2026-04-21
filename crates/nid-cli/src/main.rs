//! nid CLI binary.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod cmd;

/// nid — AI coding agent output compressor.
#[derive(Debug, Parser)]
#[command(name = "nid", version, about = "compress AI agent shell output")]
struct Cli {
    /// Run a wrapped command in shadow mode (capture both raw and compressed).
    #[arg(long, global = true)]
    shadow: bool,

    #[command(subcommand)]
    command: Option<Command>,

    /// When no subcommand is given, the remaining argv is treated as the
    /// command to compress, as if invoked by the hook. Empty for `nid` with
    /// no args (prints help).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    passthrough: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print version info.
    Version,
    /// Diagnostics: hook integrity, DB health, backends, perms.
    Doctor,
    /// Install/check/uninstall hooks and write the backup.
    Onboard(cmd::onboard::OnboardArgs),
    /// List / inspect / manage profiles.
    #[command(subcommand)]
    Profiles(cmd::profiles::ProfilesCmd),
    /// Show raw output for a prior session id.
    Show(cmd::show::ShowArgs),
    /// List recent sessions.
    Sessions(cmd::sessions::SessionsArgs),
    /// Token savings analytics.
    Gain(cmd::gain::GainArgs),
    /// Shadow-mode controls.
    #[command(subcommand)]
    Shadow(cmd::shadow::ShadowCmd),
    /// Manually trigger synthesis for a command.
    Synthesize(cmd::synthesize::SynthesizeArgs),
    /// Trust keyring for imported profiles.
    #[command(subcommand)]
    Trust(cmd::trust::TrustCmd),
    /// Garbage-collect blobs + purge old sessions.
    Gc,
    /// Update to a new nid release.
    Update(cmd::update::UpdateArgs),
    /// Internal: per-agent hook handler. Reads PreTool JSON from stdin and
    /// emits a rewrite response on stdout.
    #[command(hide = true, name = "__hook")]
    Hook {
        agent: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();

    match cli.command {
        Some(Command::Version) => cmd::version::run(),
        Some(Command::Doctor) => cmd::doctor::run().await,
        Some(Command::Onboard(args)) => cmd::onboard::run(args).await,
        Some(Command::Profiles(sub)) => cmd::profiles::run(sub).await,
        Some(Command::Show(args)) => cmd::show::run(args).await,
        Some(Command::Sessions(args)) => cmd::sessions::run(args).await,
        Some(Command::Gain(args)) => cmd::gain::run(args).await,
        Some(Command::Shadow(sub)) => cmd::shadow::run(sub).await,
        Some(Command::Synthesize(args)) => cmd::synthesize::run(args).await,
        Some(Command::Trust(sub)) => cmd::trust::run(sub).await,
        Some(Command::Gc) => cmd::gc::run().await,
        Some(Command::Update(args)) => cmd::update::run(args).await,
        Some(Command::Hook { agent }) => cmd::hook::run(agent).await,
        None => {
            if cli.passthrough.is_empty() {
                print_help();
                Ok(())
            } else {
                cmd::run::run(cli.passthrough, cli.shadow).await
            }
        }
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,nid=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn print_help() {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    let _ = cmd.print_help();
    println!();
}
