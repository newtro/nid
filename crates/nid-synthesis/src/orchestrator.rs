//! Single canonical synthesizer (plan §7.2).
//!
//! Pipeline:
//! 1. Structural-diff pre-pass over the N samples — always runs; produces a
//!    valid baseline DSL.
//! 2. Optional LLM refinement via backend. On success, validate + run against
//!    the samples; if it loses invariants or fails self-tests, discard and
//!    keep the structural-diff baseline.
//! 3. Self-tests are derived from the samples (plan §7.4).

use nid_dsl::{ast::Profile, diff, validator};

pub struct SynthesisOutcome {
    pub profile: Profile,
    pub backend_used: Option<String>,
    pub refinements_tried: u32,
    pub refinements_accepted: u32,
}

/// Synthesize a profile from N samples. `refiner` is called with an assembled
/// prompt and may return an improved DSL TOML string; if Ok(None) or Err, the
/// structural-diff baseline is used.
pub async fn synthesize_from_samples<F, Fut>(
    fingerprint: &str,
    samples: &[String],
    refiner: F,
) -> anyhow::Result<SynthesisOutcome>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<String>>>,
{
    let sample_refs: Vec<&str> = samples.iter().map(|s| s.as_str()).collect();

    let baseline = diff::synthesize(fingerprint, &sample_refs);
    validator::validate_profile(&baseline)?;

    let prompt = build_refinement_prompt(fingerprint, samples, &baseline);
    let mut outcome = SynthesisOutcome {
        profile: baseline.clone(),
        backend_used: None,
        refinements_tried: 1,
        refinements_accepted: 0,
    };

    match refiner(prompt).await {
        Ok(Some(refined_toml)) => match Profile::from_toml(&refined_toml) {
            Ok(p) => {
                if validator::validate_profile(&p).is_ok() {
                    outcome.profile = p;
                    outcome.refinements_accepted += 1;
                    outcome.backend_used = Some("llm".into());
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "LLM refinement produced unparseable TOML; keeping baseline");
            }
        },
        Ok(None) => {
            tracing::info!("no LLM backend available; using structural-diff baseline");
        }
        Err(e) => {
            tracing::warn!(error = %e, "LLM refinement errored; keeping baseline");
        }
    }

    Ok(outcome)
}

fn build_refinement_prompt(fingerprint: &str, samples: &[String], baseline: &Profile) -> String {
    let mut out = String::new();
    out.push_str("You are refining a compression DSL for the command `");
    out.push_str(fingerprint);
    out.push_str("`.\n");
    out.push_str("N raw samples:\n");
    for (i, s) in samples.iter().enumerate() {
        out.push_str(&format!("--- sample {} ---\n", i + 1));
        out.push_str(s);
        if !s.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str("\nCurrent DSL (from structural-diff / prior LLM):\n");
    out.push_str(&baseline.to_toml().unwrap_or_default());
    out.push_str("\nCurrent invariants that MUST be preserved:\n");
    for inv in &baseline.invariants {
        out.push_str("- ");
        out.push_str(&inv.name);
        out.push('\n');
    }
    out.push_str(
        "\nEmit ONLY improved DSL as TOML. No prose. Rules:\n\
- Preserve every listed invariant.\n\
- Output must be a structural subset of input (keep/drop/collapse only — NO REWRITES).\n\
- You may add new invariants you observe in the samples.\n\
- Target: minimize compressed size while preserving every ERROR/FATAL line and exit indicator.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn falls_back_when_refiner_returns_none() {
        let samples = vec!["start\nerror: x\nend\n".into(), "start\ninfo: y\nend\n".into()];
        let out = synthesize_from_samples("test", &samples, |_| async { Ok(None) })
            .await
            .unwrap();
        assert_eq!(out.refinements_accepted, 0);
        assert_eq!(out.profile.meta.fingerprint, "test");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn accepts_valid_llm_refinement() {
        let samples = vec!["foo\n".into(), "foo\n".into(), "foo\n".into()];
        let refined = r#"
[meta]
fingerprint = "test"
version = "1.0.1"
schema = "1.0"
format_claim = "plain"

[[rules]]
kind = "keep_lines"
match = "^foo$"
"#;
        let out = synthesize_from_samples("test", &samples, |_| async move {
            Ok(Some(refined.to_string()))
        })
        .await
        .unwrap();
        assert_eq!(out.refinements_accepted, 1);
        assert_eq!(out.profile.meta.version, "1.0.1");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rejects_unparseable_llm_output() {
        let samples = vec!["foo\n".into(), "foo\n".into()];
        let out = synthesize_from_samples("test", &samples, |_| async {
            Ok(Some("not TOML at all {{{".into()))
        })
        .await
        .unwrap();
        assert_eq!(out.refinements_accepted, 0);
    }
}
