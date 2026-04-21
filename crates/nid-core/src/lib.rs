//! nid-core: Compressor trait, Context, CompressionResult, dispatch primitives.
//!
//! This crate defines the shape every compression layer implements and the
//! fingerprinting (Scheme R) that the dispatcher uses to route a command to
//! a profile. No I/O, no subprocess, no filesystem.

pub mod context;
pub mod compressor;
pub mod fingerprint;
pub mod layers;
pub mod redact;
pub mod session;

pub use compressor::{
    Applicability, Compressor, CompressionResult, CompressorMode, DroppedBlock, FormatKind,
    InvariantResult,
};
pub use context::Context;
pub use fingerprint::{canonicalize_argv, fingerprint};
pub use session::{SessionId, SessionRef};
