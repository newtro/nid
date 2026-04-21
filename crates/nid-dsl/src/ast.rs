//! DSL abstract syntax (matches Appendix B grammar).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FormatClaim {
    Plain,
    Json,
    Ndjson,
    Diff,
    Log,
    Tabular,
    StackTrace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub fingerprint: String,
    pub version: String,
    #[serde(default = "default_schema")]
    pub schema: String,
    #[serde(default)]
    pub format_claim: Option<FormatClaim>,
    #[serde(default)]
    pub description: Option<String>,
}

fn default_schema() -> String {
    "1.0".to_string()
}

/// All allowed rule kinds (plan §7.1). The TOML shape uses an adjacently-tagged
/// `kind = "..."` discriminant with per-kind fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleKind {
    /// Keep lines matching `match`.
    KeepLines {
        #[serde(rename = "match")]
        match_: String,
    },
    /// Drop lines matching `match`.
    DropLines {
        #[serde(rename = "match")]
        match_: String,
    },
    /// Collapse N or more consecutive lines matching `pattern` into `placeholder`.
    CollapseRepeated {
        pattern: String,
        placeholder: String,
        #[serde(default = "default_collapse_min")]
        min: usize,
    },
    /// Collapse everything strictly between two fence patterns into `placeholder`.
    CollapseBetween {
        begin: String,
        end: String,
        placeholder: String,
    },
    /// Keep the first `n` lines only.
    Head { n: usize },
    /// Keep the last `n` lines only.
    Tail { n: usize },
    /// Keep the first `n` lines that follow the first line matching `after_match`.
    HeadAfter { n: usize, after_match: String },
    /// Keep the last `n` lines that precede the first line matching `before_match`.
    TailBefore { n: usize, before_match: String },
    /// Deduplicate adjacent identical lines.
    Dedup,
    /// Remove ANSI color/cursor control sequences.
    StripAnsi,
    /// (JSON) keep only these JSONPath-ish expressions.
    JsonPathKeep { paths: Vec<String> },
    /// (JSON) drop these JSONPath-ish expressions.
    JsonPathDrop { paths: Vec<String> },
    /// (NDJSON) keep objects matching `field == value` predicates; drop others.
    NdjsonFilter {
        field: String,
        keep_values: Vec<String>,
    },
    /// Bounded state-machine — used for section-oriented output like git status.
    StateMachine { states: Vec<StateDef> },
    /// Truncate the whole output to at most `bytes` with an elision marker.
    TruncateTo { bytes: usize },
}

fn default_collapse_min() -> usize {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDef {
    pub name: String,
    /// Regex that transitions INTO this state.
    pub enter: String,
    /// Keep every line in this state whose pattern matches one of these.
    #[serde(default)]
    pub keep: Vec<String>,
    /// Drop every line in this state whose pattern matches one of these.
    #[serde(default)]
    pub drop: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    #[serde(flatten)]
    pub kind: RuleKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case")]
pub enum InvariantCheck {
    LastLineMatches {
        pattern: String,
    },
    FirstLineMatches {
        pattern: String,
    },
    /// Every line in *input* matching `pattern` must also appear in *output*.
    AllMatchingPreserved {
        pattern: String,
    },
    /// At least `count` lines in output must match `pattern`.
    CountMatchesAtLeast {
        pattern: String,
        count: usize,
    },
    JsonPathExists {
        path: String,
    },
    ExitLinePreserved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invariant {
    pub name: String,
    #[serde(flatten)]
    pub check: InvariantCheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfTest {
    pub sample_sha256: String,
    pub expected_compressed_sha256: String,
    #[serde(default)]
    pub expected_invariants: Vec<String>,
}

/// Top-level DSL document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub meta: Meta,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub invariants: Vec<Invariant>,
    #[serde(default)]
    pub self_tests: Vec<SelfTest>,
}

impl Profile {
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_git_status_example() {
        let src = r#"
[meta]
fingerprint = "git status"
version = "1.0.0"
schema = "1.0"
format_claim = "plain"

[[rules]]
kind = "strip_ansi"

[[rules]]
kind = "keep_lines"
match = "^On branch "

[[invariants]]
name = "BranchLinePreserved"
check = "first_line_matches"
pattern = "^(On branch |HEAD detached|Your branch)"
"#;
        let p = Profile::from_toml(src).expect("parse");
        assert_eq!(p.meta.fingerprint, "git status");
        assert_eq!(p.rules.len(), 2);
        assert!(matches!(p.rules[0].kind, RuleKind::StripAnsi));
        assert!(matches!(p.rules[1].kind, RuleKind::KeepLines { .. }));
        assert_eq!(p.invariants.len(), 1);
    }

    #[test]
    fn roundtrips_via_toml() {
        let p = Profile {
            meta: Meta {
                fingerprint: "t".into(),
                version: "1.0.0".into(),
                schema: "1.0".into(),
                format_claim: Some(FormatClaim::Plain),
                description: None,
            },
            rules: vec![Rule {
                kind: RuleKind::Dedup,
            }],
            invariants: vec![],
            self_tests: vec![],
        };
        let s = p.to_toml().unwrap();
        let back = Profile::from_toml(&s).unwrap();
        assert_eq!(back.meta.fingerprint, "t");
    }
}
