//! `claude` CLI subprocess backend. Spawns the local `claude` binary with
//! the prompt on stdin; reads TOML on stdout.
//!
//! Gracefully degrades: if the binary isn't on PATH, `from_env` returns None.

use crate::backend::{Backend, BackendKind};
use std::future::Future;
use std::pin::Pin;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct ClaudeCliBackend {
    binary: String,
}

impl ClaudeCliBackend {
    pub fn from_env() -> Option<Self> {
        let bin = std::env::var("CLAUDE_CLI").unwrap_or_else(|_| "claude".to_string());
        if which(&bin).is_some() {
            Some(Self { binary: bin })
        } else {
            None
        }
    }
}

fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for p in std::env::split_paths(&path) {
        for ext in ["", ".exe", ".cmd", ".bat"] {
            let candidate = p.join(format!("{bin}{ext}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

impl Backend for ClaudeCliBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::ClaudeCli
    }

    fn refine<'a>(
        &'a self,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<String>>> + Send + 'a>> {
        Box::pin(async move {
            let mut child = Command::new(&self.binary)
                .arg("--print")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(prompt.as_bytes()).await?;
                stdin.shutdown().await?;
            }
            let out = child.wait_with_output().await?;
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                anyhow::bail!("claude CLI failed: {err}");
            }
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            if stdout.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(crate::backends::anthropic_strip_fences(&stdout)))
            }
        })
    }
}
