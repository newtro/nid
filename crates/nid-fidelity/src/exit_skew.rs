//! Exit-code correlation (plan §8.3).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkewReport {
    pub success_ratio: f32,
    pub failure_ratio: f32,
    pub skew_factor: f32,
    pub needs_restratified_resynthesis: bool,
}

/// Given sums of raw/compressed bytes split by exit-code bucket, compute the
/// compression ratio in each bucket and decide whether a re-synthesis is due.
/// If either bucket has fewer than `min_samples` runs, return a neutral
/// report (no action).
pub fn exit_code_skew(
    success_runs: u32,
    success_raw: u64,
    success_compressed: u64,
    failure_runs: u32,
    failure_raw: u64,
    failure_compressed: u64,
    min_samples: u32,
) -> SkewReport {
    if success_runs < min_samples || failure_runs < min_samples {
        return SkewReport {
            success_ratio: 0.0,
            failure_ratio: 0.0,
            skew_factor: 0.0,
            needs_restratified_resynthesis: false,
        };
    }

    let sr = ratio(success_compressed, success_raw);
    let fr = ratio(failure_compressed, failure_raw);

    // "more than 2x difference" (plan §8.3): errors compressing worse than
    // successes means failure_ratio > 2 * success_ratio.
    let skew = if sr > 0.0 { fr / sr } else { 0.0 };
    let needs = skew > 2.0;

    SkewReport {
        success_ratio: sr,
        failure_ratio: fr,
        skew_factor: skew,
        needs_restratified_resynthesis: needs,
    }
}

fn ratio(num: u64, den: u64) -> f32 {
    if den == 0 {
        0.0
    } else {
        num as f32 / den as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_min_samples() {
        let r = exit_code_skew(5, 1000, 100, 5, 1000, 100, 50);
        assert!(!r.needs_restratified_resynthesis);
    }

    #[test]
    fn flags_when_failures_compress_much_worse() {
        // Success: 10% compression ratio; failure: 80%. Skew = 8x.
        let r = exit_code_skew(100, 1000, 100, 100, 1000, 800, 50);
        assert!(r.needs_restratified_resynthesis);
        assert!((r.skew_factor - 8.0).abs() < 1e-3);
    }

    #[test]
    fn balanced_compression_is_fine() {
        let r = exit_code_skew(100, 1000, 100, 100, 1000, 110, 50);
        assert!(!r.needs_restratified_resynthesis);
    }
}
