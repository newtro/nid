//! `nid onboard` — install/check hooks (plan §10.1).

use anyhow::Result;
use clap::Args;
use nid_hooks::{detect::detect_agents, onboard};

#[derive(Debug, Args)]
pub struct OnboardArgs {
    #[arg(long)]
    pub non_interactive: bool,
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub reconfigure: bool,
    #[arg(long)]
    pub uninstall: bool,
    #[arg(long)]
    pub purge: bool,
    #[arg(long, value_delimiter = ',')]
    pub agents: Option<Vec<String>>,
    #[arg(long)]
    pub disable_synthesis: bool,
    #[arg(long)]
    pub budget: Option<f64>,
}

pub async fn run(args: OnboardArgs) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    paths.ensure()?;

    let home = directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let detected = detect_agents(&home);

    if args.check {
        println!("nid onboard --check");
        for a in &detected.agents {
            let status = if a.config_exists { "present" } else { "missing" };
            println!("  {} [{}] at {}", a.kind.display_name(), status, a.config_path.display());
        }
        println!(
            "  backends: anthropic={} ollama={} claude_cli={}",
            detected.backends.anthropic_api_key,
            detected.backends.ollama_reachable,
            detected.backends.claude_cli,
        );
        return Ok(());
    }

    let filter: Option<Vec<nid_hooks::AgentKind>> = args
        .agents
        .map(|v| v.iter().filter_map(|s| parse_agent(s)).collect());
    let opts = onboard::OnboardOptions {
        non_interactive: args.non_interactive,
        check_only: args.check,
        agents: filter,
        disable_synthesis: args.disable_synthesis,
        budget_usd: args.budget,
        preserve_raw: None,
    };

    let plan = onboard::plan(&detected, &opts, paths.onboard_backup.clone());
    println!("Planned changes:");
    for c in &plan.changes {
        println!("  {:?} -> {}", c.agent, c.config_path.display());
    }
    println!("(apply/uninstall not yet wired in Phase 1 — see Phase 2)");

    Ok(())
}

fn parse_agent(s: &str) -> Option<nid_hooks::AgentKind> {
    use nid_hooks::AgentKind::*;
    match s {
        "claude_code" | "claude-code" => Some(ClaudeCode),
        "cursor" => Some(Cursor),
        "codex_cli" | "codex-cli" | "codex" => Some(CodexCli),
        "gemini_cli" | "gemini-cli" | "gemini" => Some(GeminiCli),
        "copilot_cli" | "copilot-cli" | "copilot" => Some(CopilotCli),
        "windsurf" => Some(Windsurf),
        "opencode" => Some(OpenCode),
        "aider" => Some(Aider),
        _ => None,
    }
}
