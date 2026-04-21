//! nid-dsl: the declarative compression DSL.
//!
//! Parsed from TOML, validated against the forbidden-primitive list
//! (no IO/exec/eval, no regex backreferences), and interpreted in pure Rust.

pub mod ast;
pub mod budget;
pub mod diff;
pub mod interpreter;
pub mod invariants;
pub mod nidprofile;
pub mod validator;

pub use ast::{FormatClaim, Invariant, InvariantCheck, Profile, Rule, RuleKind};
pub use budget::{Budget, BudgetError};
pub use interpreter::{apply_rules, apply_rules_with_budget, CompressedOutput};
pub use invariants::{check_invariants, InvariantCheckError};
pub use nidprofile::{pack, unpack_and_verify, NidProfileError, UnpackedProfile};
pub use validator::{validate_profile, ValidationError};
