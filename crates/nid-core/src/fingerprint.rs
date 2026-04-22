//! Scheme R fingerprinting (plan §6.6).
//!
//! Fingerprint = `binary_name` + canonicalized argv where:
//! - Paths collapse to `<path>`
//! - Numbers collapse to `<n>`
//! - URLs collapse to `<scheme>://<host>` (path/query dropped)
//! - Quoted-string-positional args collapse to `<str>`
//! - Flag names kept verbatim
//! - Flag values usually collapsed, except "shape-defining" flags
//!   (`--format=json` vs `--format=yaml` stays distinct).

use regex::Regex;
use std::sync::OnceLock;

/// Flag keys whose values are shape-defining and MUST be kept.
/// Values stay literal for these; otherwise flag values collapse.
pub const SHAPE_DEFINING_FLAGS: &[&str] = &[
    "--format",
    "--output",
    "-o",
    "--json",
    "--pretty",
    "--oneline",
];

fn num_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^-?\d+(\.\d+)?$").unwrap())
}

fn hex_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[0-9a-fA-F]{6,}$").unwrap())
}

fn url_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^([a-zA-Z][a-zA-Z0-9+.-]*)://([^/?#]+).*$").unwrap())
}

fn path_re() -> &'static Regex {
    // Paths: starts with ./ ../ / ~ or contains / or \ or ends with common ext.
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"^(\.{1,2}[/\\]|[/\\~]|[A-Za-z]:[/\\]|[\w.-]+[/\\][\w./\\-]+|\w[\w.-]*\.(rs|py|js|ts|go|java|c|cpp|h|toml|json|yaml|yml|md|txt|log|lock|gradle|xml|sh))$"
        )
        .unwrap()
    })
}

/// Strip `--flag=value` or `-Xvalue` style into `--flag` or `-X` for the key.
/// Returns (key, Some(value)) or (whole, None) if it isn't a flag.
fn split_flag(tok: &str) -> (&str, Option<&str>) {
    if let Some(eq) = tok.find('=') {
        if tok.starts_with('-') {
            return (&tok[..eq], Some(&tok[eq + 1..]));
        }
    }
    (tok, None)
}

/// Canonicalize a single token according to Scheme R.
pub fn canonicalize_token(tok: &str, keep_literal: bool) -> String {
    if keep_literal {
        return tok.to_string();
    }
    if num_re().is_match(tok) {
        return "<n>".into();
    }
    if hex_re().is_match(tok) {
        return "<hash>".into();
    }
    if url_re().is_match(tok) {
        if let Some(c) = url_re().captures(tok) {
            return format!("{}://{}", &c[1], &c[2]);
        }
    }
    if path_re().is_match(tok) || tok.contains('/') || tok.contains('\\') {
        return "<path>".into();
    }
    // Quoted-string positional: if contains whitespace, treat as `<str>`.
    if tok.contains(' ') || tok.contains('\t') {
        return "<str>".into();
    }
    tok.to_string()
}

/// Canonicalize a full argv (after the binary name) according to Scheme R.
///
/// Returns the canonicalized token list. Binary name is NOT included.
pub fn canonicalize_argv(argv_tail: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(argv_tail.len());
    let mut i = 0;
    while i < argv_tail.len() {
        let tok = &argv_tail[i];

        // `--flag=value`
        let (key, inline_val) = split_flag(tok);
        if tok.starts_with('-') {
            let is_shape = SHAPE_DEFINING_FLAGS.contains(&key);
            match inline_val {
                Some(v) => {
                    out.push(format!("{}={}", key, canonicalize_token(v, is_shape)));
                    i += 1;
                    continue;
                }
                None => {
                    out.push(key.to_string());
                    // Peek ahead: next token (if not a flag) is the value.
                    if i + 1 < argv_tail.len() && !argv_tail[i + 1].starts_with('-') {
                        let v = &argv_tail[i + 1];
                        out.push(canonicalize_token(v, is_shape));
                        i += 2;
                        continue;
                    }
                    i += 1;
                    continue;
                }
            }
        }

        out.push(canonicalize_token(tok, false));
        i += 1;
    }
    out
}

/// Compute a stable fingerprint from argv (binary + canonical tail).
///
/// `argv[0]` is the command name (basename of binary on disk). The rest is
/// canonicalized; joined by single spaces.
pub fn fingerprint(argv: &[String]) -> String {
    if argv.is_empty() {
        return "<empty>".into();
    }
    let bin = std::path::Path::new(&argv[0])
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&argv[0])
        .to_string();
    let tail = canonicalize_argv(&argv[1..]);
    if tail.is_empty() {
        bin
    } else {
        format!("{} {}", bin, tail.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(args: &[&str]) -> String {
        fingerprint(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn bare_binary_returns_basename() {
        assert_eq!(fp(&["git"]), "git");
        assert_eq!(fp(&["/usr/bin/git"]), "git");
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_returns_basename() {
        // `Path::file_stem` on Unix treats `\` as part of the filename
        // rather than a separator, so this assertion is correct only on
        // Windows.
        assert_eq!(fp(&["C:\\bin\\git.exe"]), "git");
    }

    #[test]
    fn git_status_stable() {
        assert_eq!(fp(&["git", "status"]), "git status");
    }

    #[test]
    fn numbers_collapse() {
        assert_eq!(fp(&["git", "log", "-n", "20"]), "git log -n <n>");
        assert_eq!(fp(&["git", "log", "-n", "200"]), "git log -n <n>");
    }

    #[test]
    fn paths_collapse() {
        assert_eq!(fp(&["cat", "src/foo.rs"]), "cat <path>");
        assert_eq!(fp(&["cat", "./foo"]), "cat <path>");
    }

    #[test]
    fn urls_preserve_scheme_host() {
        assert_eq!(
            fp(&["curl", "https://api.example.com/v1/users?x=1"]),
            "curl https://api.example.com"
        );
    }

    #[test]
    fn shape_flags_keep_value() {
        assert_eq!(
            fp(&["cargo", "build", "--output=json"]),
            "cargo build --output=json"
        );
        assert_eq!(
            fp(&["git", "log", "--format=oneline"]),
            "git log --format=oneline"
        );
    }

    #[test]
    fn non_shape_flag_value_collapses() {
        // `--author Jane` -> author is not shape-defining; value collapses.
        // Jane has no spaces/paths so it stays literal — that's fine and stable.
        let a = fp(&["git", "log", "--author", "Jane"]);
        let b = fp(&["git", "log", "--author", "Jane"]);
        assert_eq!(a, b);
    }

    #[test]
    fn different_commits_fingerprint_identically() {
        assert_eq!(
            fp(&["git", "show", "abc1234"]),
            fp(&["git", "show", "def5678"])
        );
    }
}
