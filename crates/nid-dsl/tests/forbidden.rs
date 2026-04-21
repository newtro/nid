//! The DSL validator must reject any profile that attempts forbidden
//! constructs. One test per construct, per plan §11.4.

use nid_dsl::{ast::Profile, validator};

fn try_validate(toml: &str) -> Result<(), String> {
    let p = Profile::from_toml(toml).map_err(|e| format!("parse: {e}"))?;
    validator::validate_profile(&p).map_err(|e| format!("validate: {e}"))
}

#[test]
fn rejects_regex_backreferences() {
    let src = r#"
[meta]
fingerprint = "test"
version = "1.0.0"
schema = "1.0"

[[rules]]
kind = "keep_lines"
match = "(\\w+)\\1"
"#;
    let err = try_validate(src).unwrap_err();
    assert!(
        err.contains("backreference") || err.contains("Forbidden") || err.contains("forbidden"),
        "err={err}"
    );
}

#[test]
fn rejects_unsupported_schema() {
    let src = r#"
[meta]
fingerprint = "test"
version = "1.0.0"
schema = "2.0"
"#;
    let err = try_validate(src).unwrap_err();
    assert!(err.contains("schema") || err.contains("2.0"), "err={err}");
}

#[test]
fn rejects_json_path_wildcard_network_like_construct() {
    // `$.a[*].b` — we refuse wildcards for bounded-depth safety.
    let src = r#"
[meta]
fingerprint = "test"
version = "1.0.0"
schema = "1.0"

[[rules]]
kind = "json_path_keep"
paths = ["$.a[*].b"]
"#;
    let err = try_validate(src).unwrap_err();
    assert!(err.contains("unsupported"), "err={err}");
}

#[test]
fn rejects_empty_state_machine() {
    let src = r#"
[meta]
fingerprint = "test"
version = "1.0.0"
schema = "1.0"

[[rules]]
kind = "state_machine"
states = []
"#;
    let err = try_validate(src).unwrap_err();
    assert!(
        err.contains("empty") || err.contains("EmptyStateMachine"),
        "err={err}"
    );
}

#[test]
fn rejects_duplicate_state_names() {
    let src = r#"
[meta]
fingerprint = "test"
version = "1.0.0"
schema = "1.0"

[[rules]]
kind = "state_machine"
[[rules.states]]
name = "a"
enter = "^A"
[[rules.states]]
name = "a"
enter = "^B"
"#;
    let err = try_validate(src).unwrap_err();
    assert!(
        err.contains("duplicate") || err.contains("Duplicate") || err.contains("more than once"),
        "err={err}"
    );
}

#[test]
fn rejects_zero_head_or_tail() {
    for (kind, body) in [("head", "n = 0"), ("tail", "n = 0")] {
        let src = format!(
            r#"
[meta]
fingerprint = "test"
version = "1.0.0"
schema = "1.0"

[[rules]]
kind = "{kind}"
{body}
"#
        );
        let err = try_validate(&src).unwrap_err();
        assert!(err.contains("> 0"), "kind={kind} err={err}");
    }
}

#[test]
fn rejects_malformed_regex() {
    let src = r#"
[meta]
fingerprint = "test"
version = "1.0.0"
schema = "1.0"

[[rules]]
kind = "keep_lines"
match = "(unclosed"
"#;
    let err = try_validate(src).unwrap_err();
    assert!(err.contains("regex") || err.contains("Regex"), "err={err}");
}

#[test]
fn rejects_empty_fingerprint() {
    let src = r#"
[meta]
fingerprint = "   "
version = "1.0.0"
schema = "1.0"
"#;
    let err = try_validate(src).unwrap_err();
    assert!(err.contains("fingerprint"), "err={err}");
}

#[test]
fn rejects_collapse_repeated_min_too_small() {
    let src = r#"
[meta]
fingerprint = "test"
version = "1.0.0"
schema = "1.0"

[[rules]]
kind = "collapse_repeated"
pattern = "x"
placeholder = "[...]"
min = 1
"#;
    let err = try_validate(src).unwrap_err();
    assert!(err.contains("min"), "err={err}");
}
