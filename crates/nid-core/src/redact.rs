//! Secret redaction (plan §11.3).
//!
//! Runs BEFORE raw output enters the session store. Patterns are conservative
//! to minimize false positives; the high-entropy heuristic is the weakest
//! signal and gated by length + alphabet.

use regex::Regex;
use std::sync::OnceLock;

/// Named secret pattern.
pub struct Pattern {
    pub name: &'static str,
    regex: &'static OnceLock<Regex>,
    src: &'static str,
}

impl Pattern {
    fn re(&self) -> &'static Regex {
        self.regex.get_or_init(|| Regex::new(self.src).unwrap())
    }
}

macro_rules! pattern {
    ($name:expr, $src:expr) => {{
        static L: OnceLock<Regex> = OnceLock::new();
        Pattern {
            name: $name,
            regex: &L,
            src: $src,
        }
    }};
}

pub fn builtin_patterns() -> Vec<Pattern> {
    vec![
        pattern!("aws_access_key", r"AKIA[0-9A-Z]{16}"),
        pattern!("github_pat_classic", r"ghp_[A-Za-z0-9]{36,}"),
        pattern!("github_pat_fine", r"github_pat_[A-Za-z0-9_]{40,}"),
        pattern!("gitlab_token", r"glpat-[A-Za-z0-9_-]{20,}"),
        pattern!("stripe_live", r"sk_live_[A-Za-z0-9]{20,}"),
        pattern!("stripe_test", r"sk_test_[A-Za-z0-9]{20,}"),
        pattern!("jwt", r"eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_.+/=-]{10,}"),
        pattern!(
            "bearer_header",
            r"(?i)(authorization|auth)\s*:\s*bearer\s+[A-Za-z0-9._~+/=-]{20,}"
        ),
        pattern!(
            "ssh_private_key_block",
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----"
        ),
    ]
}

/// Redact built-in secret patterns from input. Each match replaced with a
/// fixed-width token tagging the pattern name so downstream can see what was
/// redacted without leaking content.
pub fn redact(input: &str) -> String {
    let mut out = input.to_string();
    for p in builtin_patterns() {
        out = p
            .re()
            .replace_all(&out, |_: &regex::Captures| format!("[REDACTED:{}]", p.name))
            .to_string();
    }

    // High-entropy heuristic: tokens of >= 32 chars from the base64url alphabet
    // that look like they pack > 4.5 bits/char of Shannon entropy get masked.
    out = high_entropy_redact(&out);
    out
}

fn shannon_entropy(s: &str) -> f64 {
    let mut counts = [0usize; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let n = s.len() as f64;
    let mut h = 0.0;
    for &c in &counts {
        if c == 0 {
            continue;
        }
        let p = c as f64 / n;
        h -= p * p.log2();
    }
    h
}

fn high_entropy_redact(input: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    // Must be a base64url / hex-ish run of >=32 chars, word-bounded.
    let re = R.get_or_init(|| Regex::new(r"\b[A-Za-z0-9_+/=-]{32,}\b").unwrap());
    re.replace_all(input, |c: &regex::Captures| {
        let s = c.get(0).unwrap().as_str();
        // Skip if already redacted or a sha256-like hex value which we'd rather keep
        // visible for blob references.
        if s.starts_with("[REDACTED:") {
            return s.to_string();
        }
        let h = shannon_entropy(s);
        if h > 4.5 {
            "[REDACTED:high_entropy]".to_string()
        } else {
            s.to_string()
        }
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_key_redacted() {
        let s = "AKIAIOSFODNN7EXAMPLE should be hidden";
        let out = redact(s);
        assert!(out.contains("[REDACTED:aws_access_key]"), "out={out}");
    }

    #[test]
    fn github_pat_classic_redacted() {
        let s = "token ghp_1234567890abcdefghij1234567890abcdefghij here";
        let out = redact(s);
        assert!(out.contains("[REDACTED:github_pat_classic]"), "out={out}");
    }

    #[test]
    fn github_pat_fine_redacted() {
        let s = "token github_pat_1234567890abcdefghij1234567890abcdefghij1234567 here";
        let out = redact(s);
        assert!(out.contains("[REDACTED:github_pat_fine]"), "out={out}");
    }

    #[test]
    fn gitlab_token_redacted() {
        let s = "glpat-abcdefghij1234567890 here";
        let out = redact(s);
        assert!(out.contains("[REDACTED:gitlab_token]"), "out={out}");
    }

    #[test]
    fn stripe_live_redacted() {
        let s = "key sk_live_abcdefghij1234567890 here";
        let out = redact(s);
        assert!(out.contains("[REDACTED:stripe_live]"), "out={out}");
    }

    #[test]
    fn stripe_test_redacted() {
        let s = "key sk_test_abcdefghij1234567890 here";
        let out = redact(s);
        assert!(out.contains("[REDACTED:stripe_test]"), "out={out}");
    }

    #[test]
    fn jwt_redacted() {
        let s = "token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf here";
        let out = redact(s);
        assert!(out.contains("[REDACTED:jwt]"), "out={out}");
    }

    #[test]
    fn bearer_token_redacted() {
        let s = "Authorization: Bearer abcdef1234567890abcdef1234567890\n";
        let out = redact(s);
        assert!(out.contains("[REDACTED:bearer_header]"), "out={out}");
    }

    #[test]
    fn ssh_private_key_redacted() {
        let s = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc123def\n-----END OPENSSH PRIVATE KEY-----";
        let out = redact(s);
        assert!(out.contains("[REDACTED:ssh_private_key_block]"), "out={out}");
    }

    #[test]
    fn innocuous_text_not_redacted() {
        let s = "Compiling hello 0.1.0\n   Finished `dev` in 3.2s";
        let out = redact(s);
        assert_eq!(out, s);
    }
}
