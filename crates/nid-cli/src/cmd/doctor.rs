//! `nid doctor` — diagnostics (plan §4.1).

use anyhow::Result;
use nid_storage::Db;

pub async fn run() -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    println!("nid doctor");
    println!("  config dir: {}", paths.config_dir.display());
    println!("  data dir:   {}", paths.data_dir.display());

    // DB health
    match Db::open(&paths.db_path) {
        Ok(db) => {
            let v = db.schema_version().unwrap_or(0);
            println!("  sqlite:     OK (schema v{v})");
        }
        Err(e) => {
            println!("  sqlite:     FAIL — {e}");
        }
    }

    // Backends
    let has_anth = std::env::var_os("ANTHROPIC_API_KEY").is_some();
    println!(
        "  backends:   ANTHROPIC_API_KEY={} ollama={} claude={}",
        yesno(has_anth),
        yesno(on_path("ollama")),
        yesno(on_path("claude"))
    );

    Ok(())
}

fn yesno(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

fn on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for p in std::env::split_paths(&path) {
        for ext in ["", ".exe", ".cmd", ".bat"] {
            if p.join(format!("{bin}{ext}")).exists() {
                return true;
            }
        }
    }
    false
}
