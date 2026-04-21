//! nid-dsl: the declarative compression DSL.
//!
//! Parsed from TOML, validated against the forbidden-primitive list
//! (no IO/exec/eval, no regex backreferences), and interpreted in pure Rust.

pub mod ast;
pub mod diff;
pub mod interpreter;
pub mod invariants;
pub mod validator;

pub use ast::{FormatClaim, Invariant, InvariantCheck, Profile, Rule, RuleKind};
pub use interpreter::{apply_rules, CompressedOutput};
pub use invariants::{check_invariants, InvariantCheckError};
pub use validator::{validate_profile, ValidationError};
