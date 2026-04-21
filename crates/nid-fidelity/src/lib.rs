//! nid-fidelity: invariant checks, structural checks, bypass signals, exit-skew.
//!
//! Plan §8 — four-tier scoring + six-signal bypass detection.

pub mod bypass;
pub mod exit_skew;
pub mod structural;

pub use bypass::{BypassSignal, BypassTracker};
pub use exit_skew::{exit_code_skew, SkewReport};
pub use structural::{structural_subset_check, StructuralResult};
