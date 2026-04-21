//! `nid gain` — token/dollar savings analytics (plan §4.1).
//!
//! Two output modes:
//! - default: totals + per-fingerprint breakdown sorted by tokens saved.
//! - `--shadow`: same but filtered to Shadow-mode sessions (counterfactual).

use anyhow::Result;
use clap::Args;
use nid_storage::{session_repo::SessionRepo, Db};
use std::collections::BTreeMap;

#[derive(Debug, Args)]
pub struct GainArgs {
    #[arg(long)]
    pub shadow: bool,
    /// Cost per million input tokens in USD for the model you're estimating
    /// against. Default: Opus 4.7 input-token price ($15/M as of 2026-04).
    #[arg(long, default_value_t = 15.0)]
    pub per_million_usd: f64,
    /// How many top fingerprints to print. Default 20.
    #[arg(long, default_value_t = 20)]
    pub top: usize,
}

#[derive(Default, Clone)]
struct Accumulator {
    runs: i64,
    raw_bytes: i64,
    compressed_bytes: i64,
}

impl Accumulator {
    fn add(&mut self, raw: i64, cmp: i64) {
        self.runs += 1;
        self.raw_bytes += raw;
        self.compressed_bytes += cmp;
    }
    fn tokens_saved(&self) -> i64 {
        (self.raw_bytes - self.compressed_bytes).max(0) / 4
    }
    fn usd_saved(&self, per_million: f64) -> f64 {
        self.tokens_saved() as f64 * per_million / 1_000_000.0
    }
    fn ratio(&self) -> f64 {
        if self.raw_bytes == 0 {
            0.0
        } else {
            self.compressed_bytes as f64 / self.raw_bytes as f64
        }
    }
}

pub async fn run(args: GainArgs) -> Result<()> {
    let paths = crate::cmd::paths::resolve()?;
    let db = Db::open(&paths.db_path)?;
    let repo = SessionRepo::new(&db);
    let rows = repo.list_recent(1_000_000)?;

    let mut total = Accumulator::default();
    let mut by_fp: BTreeMap<String, Accumulator> = BTreeMap::new();
    for r in &rows {
        if args.shadow {
            if r.mode.as_deref() != Some("Shadow") {
                continue;
            }
        } else if r.mode.as_deref() == Some("Shadow") {
            // Default excludes shadow rows (they're counterfactual).
            continue;
        }
        let raw = r.raw_bytes.unwrap_or(0);
        let cmp = r.compressed_bytes.unwrap_or(0);
        total.add(raw, cmp);
        by_fp
            .entry(r.fingerprint.clone())
            .or_default()
            .add(raw, cmp);
    }

    let mode_label = if args.shadow {
        "SHADOW (counterfactual)"
    } else {
        "LIVE"
    };
    println!("--- nid gain [{mode_label}] ---");
    println!(
        "runs: {runs}  raw: {raw}B  compressed: {cmp}B  ratio: {ratio:.2}  saved ~{tok} tokens (~${usd:.2} @ ${per:.2}/M)",
        runs = total.runs,
        raw = total.raw_bytes,
        cmp = total.compressed_bytes,
        ratio = total.ratio(),
        tok = total.tokens_saved(),
        usd = total.usd_saved(args.per_million_usd),
        per = args.per_million_usd,
    );

    if by_fp.is_empty() {
        return Ok(());
    }

    let mut fps: Vec<(String, Accumulator)> = by_fp.into_iter().collect();
    fps.sort_by(|a, b| b.1.tokens_saved().cmp(&a.1.tokens_saved()));

    println!("\ntop {} fingerprints by tokens saved:", args.top);
    for (fp, acc) in fps.into_iter().take(args.top) {
        println!(
            "  {fp:40}  runs={runs:>5}  ratio={ratio:.2}  saved~{tok} tok (~${usd:.2})",
            runs = acc.runs,
            ratio = acc.ratio(),
            tok = acc.tokens_saved(),
            usd = acc.usd_saved(args.per_million_usd),
        );
    }
    Ok(())
}
