//! The command-rewrite rules (plan §4.4) — agent-agnostic.
//!
//! This is the meat of what hooks do, factored out so every agent's handler
//! goes through one implementation.

use serde::{Deserialize, Serialize};

/// Shell builtins that must never be wrapped — `nid cd ..` doesn't do anything
/// useful (and would swallow the cd in a subshell).
pub const BUILTINS: &[&str] = &[
    "cd", "export", "set", "unset", "alias", "source", ".", "eval", "pwd", "echo", "printf",
    "read", "exit", "return",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RewriteDecision {
    /// Do not touch the command; pass through as-is.
    Passthrough {
        reason: &'static str,
        original: String,
    },
    /// Prepend `nid` with optional flags (e.g. `--shadow`).
    Rewritten { updated: String, original: String },
}

#[derive(Debug, Clone, Default)]
pub struct RewriteOptions {
    pub shadow: bool,
    /// Additional passthrough-pattern regexes supplied via config.
    pub extra_passthrough: Vec<String>,
}

/// Rewrite decision for a single incoming command string.
pub fn rewrite_command(cmd: &str, opts: &RewriteOptions) -> RewriteDecision {
    let trimmed = cmd.trim_start();

    // Rule 4: NID_RAW=1 escape hatch.
    if let Some(rest) = trimmed
        .strip_prefix("NID_RAW=1 ")
        .or(trimmed.strip_prefix("NID_RAW=1\t"))
    {
        return RewriteDecision::Passthrough {
            reason: "nid_raw_escape",
            original: rest.to_string(),
        };
    }

    // Rule 1: idempotent. If the command already starts with `nid`, leave it.
    if starts_with_nid(trimmed) {
        return RewriteDecision::Passthrough {
            reason: "already_nid",
            original: cmd.to_string(),
        };
    }

    // Rule 3: passthrough list — builtins and pipeline plumbing.
    if first_token_is_builtin(trimmed) {
        return RewriteDecision::Passthrough {
            reason: "builtin",
            original: cmd.to_string(),
        };
    }
    if is_pure_pipeline_plumbing(trimmed) {
        return RewriteDecision::Passthrough {
            reason: "pipeline_plumbing",
            original: cmd.to_string(),
        };
    }

    // Rule 3 cont.: user-specified passthrough patterns.
    for pat in &opts.extra_passthrough {
        if let Ok(re) = regex::Regex::new(pat) {
            if re.is_match(trimmed) {
                return RewriteDecision::Passthrough {
                    reason: "user_passthrough",
                    original: cmd.to_string(),
                };
            }
        }
    }

    // Rule 2: whole-pipeline wrap. Don't split on `|`; the shell does that.
    let prefix = if opts.shadow { "nid --shadow " } else { "nid " };
    RewriteDecision::Rewritten {
        updated: format!("{prefix}{cmd}"),
        original: cmd.to_string(),
    }
}

fn starts_with_nid(s: &str) -> bool {
    // Match `nid`, `nid ...`, `nid --flag ...`. Don't match a path ending in
    // `nid` (e.g. `/usr/local/bin/nid`) — treat those as already-nid too.
    if s.starts_with("nid ") || s == "nid" {
        return true;
    }
    // Also detect absolute/relative paths that end with `nid` or `nid.exe`.
    if let Some(first) = s.split_ascii_whitespace().next() {
        if first.ends_with("/nid")
            || first.ends_with("/nid.exe")
            || first.ends_with("\\nid")
            || first.ends_with("\\nid.exe")
        {
            return true;
        }
        // Windows: any path that resolves to the nid binary by basename.
        let base = std::path::Path::new(first)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if base.eq_ignore_ascii_case("nid") {
            return true;
        }
    }
    false
}

fn first_token(s: &str) -> Option<&str> {
    s.split_ascii_whitespace().next()
}

fn first_token_is_builtin(s: &str) -> bool {
    match first_token(s) {
        Some(t) => BUILTINS.contains(&t),
        None => true, // empty command — passthrough
    }
}

fn is_pure_pipeline_plumbing(s: &str) -> bool {
    // Match commands where the entire point is to route bytes: `tee` / `cat`
    // with `<` `>` `|` present anywhere.
    let Some(first) = first_token(s) else {
        return false;
    };
    if (first == "tee" || first == "cat") && (s.contains('>') || s.contains('|') || s.contains('<'))
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> RewriteOptions {
        RewriteOptions::default()
    }

    #[test]
    fn wraps_a_plain_command() {
        let r = rewrite_command("pytest -v", &opts());
        match r {
            RewriteDecision::Rewritten { updated, original } => {
                assert_eq!(updated, "nid pytest -v");
                assert_eq!(original, "pytest -v");
            }
            other => panic!("expected rewrite, got {:?}", other),
        }
    }

    #[test]
    fn idempotent_for_existing_nid_prefix() {
        let r = rewrite_command("nid pytest -v", &opts());
        assert!(matches!(r, RewriteDecision::Passthrough { .. }));
    }

    #[test]
    fn idempotent_for_nid_path_prefix() {
        let r = rewrite_command("/usr/local/bin/nid pytest", &opts());
        assert!(matches!(r, RewriteDecision::Passthrough { .. }));
        let r = rewrite_command("C:\\bin\\nid.exe pytest", &opts());
        assert!(matches!(r, RewriteDecision::Passthrough { .. }));
    }

    #[test]
    fn does_not_double_wrap() {
        let first = rewrite_command("pytest", &opts());
        let updated = match first {
            RewriteDecision::Rewritten { updated, .. } => updated,
            _ => panic!(),
        };
        let second = rewrite_command(&updated, &opts());
        assert!(matches!(second, RewriteDecision::Passthrough { .. }));
    }

    #[test]
    fn skips_builtins() {
        for b in ["cd /tmp", "export X=1", "alias foo=bar", "source env.sh"] {
            assert!(
                matches!(
                    rewrite_command(b, &opts()),
                    RewriteDecision::Passthrough { .. }
                ),
                "{b} should passthrough"
            );
        }
    }

    #[test]
    fn nid_raw_escape_unwraps() {
        let r = rewrite_command("NID_RAW=1 pytest -v", &opts());
        match r {
            RewriteDecision::Passthrough { original, reason } => {
                assert_eq!(original, "pytest -v");
                assert_eq!(reason, "nid_raw_escape");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn tee_with_redirect_passthrough() {
        let r = rewrite_command("tee log.txt > out", &opts());
        assert!(matches!(r, RewriteDecision::Passthrough { .. }));
    }

    #[test]
    fn pipeline_is_wrapped_as_whole() {
        let r = rewrite_command("pytest | tee log.txt | grep FAIL", &opts());
        match r {
            RewriteDecision::Rewritten { updated, .. } => {
                assert_eq!(updated, "nid pytest | tee log.txt | grep FAIL");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn shadow_mode_uses_shadow_prefix() {
        let mut o = opts();
        o.shadow = true;
        let r = rewrite_command("cargo build", &o);
        match r {
            RewriteDecision::Rewritten { updated, .. } => {
                assert_eq!(updated, "nid --shadow cargo build");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn user_passthrough_pattern_matches() {
        let mut o = opts();
        o.extra_passthrough.push(r"^secret-cmd\b".into());
        let r = rewrite_command("secret-cmd --run", &o);
        assert!(matches!(r, RewriteDecision::Passthrough { .. }));
    }
}
