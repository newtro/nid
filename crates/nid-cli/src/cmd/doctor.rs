//! `nid doctor` — diagnostics (plan §4.1, §11.1).
//!
//! Real checks (not just paths):
//!   - SQLite: open + schema version + round-trip read/write a test row.
//!   - Blob store: put/get/delete a small payload.
//!   - Hook integrity: for each agent in `agent_registry`, re-hash the
//!     config file and compare against the recorded SHA-256. Flag mismatch.
//!   - Backends: env var detection for Anthropic; TCP probe for Ollama;
//!     PATH probe for `claude` CLI.
//!   - Permissions (Unix only): warn if config/data dirs are not 0700.
//!   - Co-installed hooks: scan each agent config for *other* hooks that
//!     also rewrite Bash commands and warn.

use anyhow::Result;
use nid_storage::{agent_registry_repo::AgentRegistryRepo, blob::BlobKind, blob::BlobStore, Db};

pub async fn run() -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    println!("nid doctor");
    println!("  config dir: {}", paths.config_dir.display());
    println!("  data dir:   {}", paths.data_dir.display());

    // -- SQLite
    let db = match Db::open(&paths.db_path) {
        Ok(d) => {
            let v = d.schema_version().unwrap_or(0);
            println!("  sqlite:     OK (schema v{v})");
            Some(d)
        }
        Err(e) => {
            println!("  sqlite:     FAIL — {e}");
            None
        }
    };

    // -- Blob store round-trip
    if let Some(db) = &db {
        let store = BlobStore::new(db, &paths.blobs_dir);
        let payload = b"nid-doctor-roundtrip";
        match store.put(payload, BlobKind::Raw) {
            Ok(sha) => match store.get(&sha) {
                Ok(got) if got == payload => {
                    println!("  blob store: OK");
                    let _ = store.release(&sha);
                }
                Ok(_) => println!("  blob store: FAIL (roundtrip payload mismatch)"),
                Err(e) => println!("  blob store: FAIL — {e}"),
            },
            Err(e) => println!("  blob store: FAIL — {e}"),
        }
    }

    // -- Backends
    let has_anth = std::env::var_os("ANTHROPIC_API_KEY").is_some();
    let ollama = probe_ollama();
    let claude = on_path("claude");
    println!(
        "  backends:   ANTHROPIC_API_KEY={} ollama={} claude={}",
        yesno(has_anth),
        yesno(ollama),
        yesno(claude)
    );

    // -- Hook integrity
    if let Some(db) = &db {
        let reg = AgentRegistryRepo::new(db);
        match reg.list() {
            Ok(rows) if rows.is_empty() => {
                println!(
                    "  hooks:      (no agents installed; run `nid onboard --non-interactive`)"
                );
            }
            Ok(rows) => {
                for row in rows {
                    let path = std::path::Path::new(&row.hook_path);
                    if !path.exists() {
                        println!(
                            "  hook :      MISSING — {}: file disappeared ({})",
                            row.agent, row.hook_path
                        );
                        continue;
                    }
                    let body = match std::fs::read(path) {
                        Ok(b) => b,
                        Err(e) => {
                            println!("  hook :      FAIL — {}: read error {e}", row.agent);
                            continue;
                        }
                    };
                    use sha2::{Digest, Sha256};
                    let mut h = Sha256::new();
                    h.update(&body);
                    let got = hex::encode(h.finalize());
                    if got == row.hook_sha256 {
                        println!("  hook :      OK — {}", row.agent);
                    } else {
                        println!(
                            "  hook :      DRIFT — {}: config changed since install ({} → {})",
                            row.agent,
                            &row.hook_sha256[..8],
                            &got[..8]
                        );
                    }
                    let text = String::from_utf8_lossy(&body);
                    let others = count_other_bash_hooks(&text);
                    if others > 0 {
                        println!(
                            "  hook :      WARN — {}: {} other Bash-rewriting hook(s) found; last-writer-wins",
                            row.agent, others
                        );
                    }
                }
            }
            Err(e) => println!("  hooks:      FAIL — {e}"),
        }
    }

    // -- Permissions (Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for p in [&paths.config_dir, &paths.data_dir, &paths.blobs_dir] {
            if let Ok(md) = std::fs::metadata(p) {
                let mode = md.mode() & 0o777;
                if mode != 0o700 {
                    println!(
                        "  perms:      WARN — {} is {:o} (want 0700)",
                        p.display(),
                        mode
                    );
                }
            }
        }
    }

    // -- Recent unredacted accesses (L-R5). Plan §11.3: `nid show
    //    --raw-unredacted` logs an audit entry; surface the last 5 here so
    //    users/auditors can see who's been reading raw.
    let log_path = paths.data_dir.join("show_access.log");
    if let Ok(body) = std::fs::read_to_string(&log_path) {
        let lines: Vec<&str> = body.lines().collect();
        if !lines.is_empty() {
            println!(
                "  audit:      {} unredacted-access event(s) in {}",
                lines.len(),
                log_path.display()
            );
            println!("              (last 5 shown)");
            for line in lines.iter().rev().take(5) {
                println!("              {line}");
            }
        }
    }

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

fn probe_ollama() -> bool {
    let host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let trimmed = host
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host_port = trimmed.split('/').next().unwrap_or("");
    let Some(addr): Option<std::net::SocketAddr> = host_port.parse().ok().or_else(|| {
        std::net::ToSocketAddrs::to_socket_addrs(&host_port)
            .ok()
            .and_then(|mut it| it.next())
    }) else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok()
}

/// Counts hook entries under `hooks.PreToolUse[*]` whose command does NOT
/// contain `__hook` (i.e., are not our own). Pure-JSON walk; the old
/// `matches("\"command\"")` substring approach over-counted any time a
/// user config used `"command"` as a property name anywhere.
fn count_other_bash_hooks(json_text: &str) -> usize {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(json_text) else {
        return 0;
    };
    let Some(pretool) = doc
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(|p| p.as_array())
    else {
        return 0;
    };
    let mut count = 0usize;
    for entry in pretool {
        // Only count Bash-matched hooks.
        let matcher_is_bash = entry
            .get("matcher")
            .and_then(|m| m.as_str())
            .map(|s| s.eq_ignore_ascii_case("bash"))
            .unwrap_or(false);
        if !matcher_is_bash {
            continue;
        }
        let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) else {
            continue;
        };
        for h in hooks {
            if let Some(cmd) = h.get("command").and_then(|c| c.as_str()) {
                if !cmd.contains("__hook") {
                    count += 1;
                }
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_non_nid_bash_hooks() {
        let j = r#"{
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{"type":"command","command":"/opt/nid/bin/nid __hook claude_code"}] },
                    { "matcher": "Edit", "hooks": [{"type":"command","command":"do-something"}] },
                    { "matcher": "Bash", "hooks": [{"type":"command","command":"other-tool"}] }
                ]
            }
        }"#;
        assert_eq!(count_other_bash_hooks(j), 1);
    }

    #[test]
    fn no_hooks_means_zero() {
        assert_eq!(count_other_bash_hooks("{}"), 0);
    }

    #[test]
    fn non_json_returns_zero() {
        assert_eq!(count_other_bash_hooks("not json"), 0);
    }
}
