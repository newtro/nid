//! Bypass detection (plan §8.2).
//!
//! Six weighted signals aggregate over a rolling 100-run window per profile.
//! Warmup window: first 3 runs after profile activation ignored.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BypassSignal {
    RawReFetch,
    NidShowRaw,
    ScriptToDiskThenRun,
    GrepAfterRead,
    NearDuplicateReInvocation,
    NidRawEnv,
}

impl BypassSignal {
    pub fn weight(&self) -> f32 {
        match self {
            BypassSignal::RawReFetch => 0.9,
            BypassSignal::NidShowRaw => 0.7,
            BypassSignal::ScriptToDiskThenRun => 0.6,
            BypassSignal::GrepAfterRead => 0.5,
            BypassSignal::NearDuplicateReInvocation => 0.4,
            BypassSignal::NidRawEnv => 0.2,
        }
    }
}

/// Rolling-window bypass score per profile.
#[derive(Debug, Clone)]
pub struct BypassTracker {
    window: usize,
    warmup: usize,
    observed: usize,
    runs: VecDeque<f32>,
}

impl BypassTracker {
    pub fn new(window: usize, warmup: usize) -> Self {
        Self {
            window,
            warmup,
            observed: 0,
            runs: VecDeque::with_capacity(window),
        }
    }

    pub fn observe(&mut self, signals: &[BypassSignal]) {
        self.observed += 1;
        if self.observed <= self.warmup {
            return;
        }
        let score: f32 = signals.iter().map(|s| s.weight()).sum::<f32>().min(1.0);
        if self.runs.len() == self.window {
            self.runs.pop_front();
        }
        self.runs.push_back(score);
    }

    /// Weighted-average score over the window, in [0.0, 1.0].
    pub fn score(&self) -> f32 {
        if self.runs.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.runs.iter().sum();
        sum / self.runs.len() as f32
    }

    pub fn exceeds(&self, threshold: f32) -> bool {
        self.score() > threshold
    }

    pub fn runs_observed(&self) -> usize {
        self.observed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_match_plan() {
        assert!((BypassSignal::RawReFetch.weight() - 0.9).abs() < 1e-6);
        assert!((BypassSignal::NidShowRaw.weight() - 0.7).abs() < 1e-6);
        assert!((BypassSignal::NidRawEnv.weight() - 0.2).abs() < 1e-6);
    }

    #[test]
    fn warmup_ignores_first_runs() {
        let mut t = BypassTracker::new(10, 3);
        for _ in 0..3 {
            t.observe(&[BypassSignal::RawReFetch]);
        }
        assert_eq!(t.score(), 0.0, "warmup must be ignored");
        t.observe(&[BypassSignal::RawReFetch]);
        assert!(t.score() > 0.0);
    }

    #[test]
    fn threshold_exceed() {
        let mut t = BypassTracker::new(5, 0);
        for _ in 0..5 {
            t.observe(&[BypassSignal::RawReFetch]);
        }
        assert!(t.exceeds(0.3));
        assert!(!t.exceeds(0.95));
    }
}
