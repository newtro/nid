//! Lock-in policy (plan §7.3).
//!
//! - N=5 default.
//! - N=3 fast-path when all samples are structurally identical (zero variance).
//! - Doubling re-refinement at 5, 10, 20, 40, ...

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockinVerdict {
    pub should_lock: bool,
    pub reason: &'static str,
}

pub fn should_lock_in(samples: &[String], default_n: usize, fast_path_zero_variance: bool) -> LockinVerdict {
    if samples.len() >= default_n {
        return LockinVerdict {
            should_lock: true,
            reason: "reached_n",
        };
    }
    if fast_path_zero_variance && samples.len() >= 3 && zero_variance(samples) {
        return LockinVerdict {
            should_lock: true,
            reason: "zero_variance_fast_path",
        };
    }
    LockinVerdict {
        should_lock: false,
        reason: "accumulating",
    }
}

/// All samples are byte-identical.
pub fn zero_variance(samples: &[String]) -> bool {
    samples.windows(2).all(|w| w[0] == w[1])
}

/// Next refinement checkpoint: 5, 10, 20, 40, ... A sample count of N>=5 triggers
/// refinement when N equals a power-of-two multiple of 5 (or equivalently, N/5
/// is a power of two).
pub fn is_doubling_checkpoint(samples: usize, base_n: usize) -> bool {
    if samples < base_n {
        return false;
    }
    let ratio = samples / base_n;
    if samples % base_n != 0 {
        return false;
    }
    // power of two
    ratio != 0 && (ratio & (ratio - 1)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("sample {i}")).collect()
    }

    #[test]
    fn locks_at_n_default() {
        let v = should_lock_in(&s(5), 5, true);
        assert!(v.should_lock);
        assert_eq!(v.reason, "reached_n");
    }

    #[test]
    fn fast_path_triggers_at_3_when_identical() {
        let same = vec!["x".into(), "x".into(), "x".into()];
        let v = should_lock_in(&same, 5, true);
        assert!(v.should_lock);
        assert_eq!(v.reason, "zero_variance_fast_path");
    }

    #[test]
    fn fast_path_skipped_when_variance_present() {
        let diff = vec!["x".into(), "y".into(), "x".into()];
        let v = should_lock_in(&diff, 5, true);
        assert!(!v.should_lock);
    }

    #[test]
    fn doubling_checkpoints_are_5_10_20_40() {
        for n in [5, 10, 20, 40, 80] {
            assert!(is_doubling_checkpoint(n, 5), "{n} should be a checkpoint");
        }
        for n in [6, 7, 15, 25, 30] {
            assert!(!is_doubling_checkpoint(n, 5), "{n} should not be a checkpoint");
        }
    }
}
