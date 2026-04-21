//! Per-run execution budget for the DSL interpreter (plan §11.4 / Appendix B).
//!
//! Enforced fields:
//! - `max_steps`: each line visited by any rule counts as one step. Exceed →
//!   abort.
//! - `max_wallclock_ms`: checked every 1024 steps.
//! - `max_peak_bytes`: rough live-bytes accounting across all `Line` bodies
//!   currently held in the interpreter. Exceed → abort.
//!
//! Abort surfaces as `BudgetError::Exceeded`. Callers degrade (typically to
//! Layer-1-only output) instead of failing the wrapped command.

use std::time::Instant;
use thiserror::Error;

pub const DEFAULT_MAX_STEPS: u64 = 10_000_000;
pub const DEFAULT_MAX_WALLCLOCK_MS: u64 = 2_000;
pub const DEFAULT_MAX_PEAK_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub max_steps: u64,
    pub max_wallclock_ms: u64,
    pub max_peak_bytes: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_steps: DEFAULT_MAX_STEPS,
            max_wallclock_ms: DEFAULT_MAX_WALLCLOCK_MS,
            max_peak_bytes: DEFAULT_MAX_PEAK_BYTES,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BudgetError {
    #[error("DSL budget exceeded: {0}")]
    Exceeded(&'static str),
}

/// Running budget counter. Cheap to construct; updated by the interpreter
/// as it processes lines. `check()` is called every N steps.
pub struct BudgetRunner {
    budget: Budget,
    started: Instant,
    steps: u64,
    peak_bytes: u64,
}

impl BudgetRunner {
    pub fn new(budget: Budget) -> Self {
        Self {
            budget,
            started: Instant::now(),
            steps: 0,
            peak_bytes: 0,
        }
    }

    /// Call this once per processed line.
    pub fn tick(&mut self) -> Result<(), BudgetError> {
        self.steps += 1;
        if self.steps > self.budget.max_steps {
            return Err(BudgetError::Exceeded("max_steps"));
        }
        // Check wallclock every 1024 steps — reading Instant::now is cheap
        // on modern kernels but not free.
        if self.steps % 1024 == 0 {
            let elapsed = self.started.elapsed().as_millis() as u64;
            if elapsed > self.budget.max_wallclock_ms {
                return Err(BudgetError::Exceeded("max_wallclock_ms"));
            }
        }
        Ok(())
    }

    /// Update peak memory accounting from the current `Vec<Line>` snapshot.
    pub fn observe_bytes(&mut self, total_bytes: usize) -> Result<(), BudgetError> {
        let b = total_bytes as u64;
        if b > self.peak_bytes {
            self.peak_bytes = b;
        }
        if self.peak_bytes > self.budget.max_peak_bytes {
            return Err(BudgetError::Exceeded("max_peak_bytes"));
        }
        Ok(())
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }
    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_exceeds_max_steps() {
        let mut r = BudgetRunner::new(Budget {
            max_steps: 3,
            max_wallclock_ms: 10_000,
            max_peak_bytes: u64::MAX,
        });
        for _ in 0..3 {
            r.tick().unwrap();
        }
        let err = r.tick().unwrap_err();
        assert_eq!(err, BudgetError::Exceeded("max_steps"));
    }

    #[test]
    fn observe_bytes_exceeds() {
        let mut r = BudgetRunner::new(Budget {
            max_steps: u64::MAX,
            max_wallclock_ms: u64::MAX,
            max_peak_bytes: 1024,
        });
        r.observe_bytes(512).unwrap();
        let err = r.observe_bytes(2048).unwrap_err();
        assert_eq!(err, BudgetError::Exceeded("max_peak_bytes"));
    }

    #[test]
    fn wallclock_honoured_within_tick_granularity() {
        let mut r = BudgetRunner::new(Budget {
            max_steps: u64::MAX,
            max_wallclock_ms: 1,
            max_peak_bytes: u64::MAX,
        });
        std::thread::sleep(std::time::Duration::from_millis(10));
        // Need to hit the 1024-tick checkpoint.
        let mut err = None;
        for _ in 0..2048 {
            if let Err(e) = r.tick() {
                err = Some(e);
                break;
            }
        }
        assert!(matches!(
            err,
            Some(BudgetError::Exceeded("max_wallclock_ms"))
        ));
    }
}
