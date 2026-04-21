//! `nid __hook <agent>` — the per-agent hook handler entry point.
//!
//! Reads JSON from stdin, runs the common rewrite logic, writes JSON to
//! stdout. This is what agent hook configs spawn.

use anyhow::Result;
use nid_hooks::{agents::handle_payload, PreToolPayload};
use std::io::{Read, Write};

pub async fn run(agent: String) -> Result<()> {
    let _agent = agent; // retained for future per-agent payload shapes
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let payload: PreToolPayload = extract_payload(&buf)?;

    // Load user config so hook.passthrough_patterns is honoured per plan §4.4.3.
    let passthrough = match crate::cmd::paths::resolve() {
        Ok(p) => {
            nid_storage::config::load(&p.config_dir)
                .hook
                .passthrough_patterns
        }
        Err(_) => Vec::new(),
    };
    // Shadow flag from disk so the hook rewrites to `nid --shadow <cmd>` when
    // enabled. Fixes H3.
    let shadow_on = match crate::cmd::paths::resolve() {
        Ok(p) => crate::cmd::shadow::is_shadow_enabled(&p.config_dir),
        Err(_) => false,
    };
    let payload = PreToolPayload {
        shadow: payload.shadow || shadow_on,
        ..payload
    };

    let resp = handle_payload(&payload, &passthrough);
    let out = serde_json::to_string(&resp)?;
    std::io::stdout().write_all(out.as_bytes())?;
    std::io::stdout().write_all(b"\n")?;
    Ok(())
}

/// Accept several input shapes:
/// - Claude Code's `{"tool_name":"Bash","tool_input":{"command":"..."}}`
/// - Generic `{"tool_name":"bash","command":"..."}`
/// - The internal `PreToolPayload` JSON directly.
fn extract_payload(buf: &str) -> Result<PreToolPayload> {
    let v: serde_json::Value = serde_json::from_str(buf)?;

    if let Ok(p) = serde_json::from_value::<PreToolPayload>(v.clone()) {
        if !p.tool_name.is_empty() {
            return Ok(p);
        }
    }
    let tool_name = v
        .get("tool_name")
        .and_then(|x| x.as_str())
        .unwrap_or("Bash")
        .to_string();

    let command = v
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(|c| c.as_str())
        .or_else(|| v.get("command").and_then(|c| c.as_str()))
        .or_else(|| {
            v.get("modifiedArgs")
                .and_then(|m| m.get("command"))
                .and_then(|c| c.as_str())
        })
        .unwrap_or("")
        .to_string();

    let shadow = v.get("shadow").and_then(|b| b.as_bool()).unwrap_or(false);
    Ok(PreToolPayload {
        tool_name,
        command,
        shadow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_claude_code_payload() {
        let s = r#"{"tool_name":"Bash","tool_input":{"command":"pytest -v"}}"#;
        let p = extract_payload(s).unwrap();
        assert_eq!(p.tool_name, "Bash");
        assert_eq!(p.command, "pytest -v");
    }

    #[test]
    fn extracts_generic_payload() {
        let s = r#"{"tool_name":"bash","command":"cargo build"}"#;
        let p = extract_payload(s).unwrap();
        assert_eq!(p.command, "cargo build");
    }
}
