//! nid-synthesis: LLM backends + synthesis orchestration.

pub mod backend;
pub mod backends;
pub mod lockin;
pub mod orchestrator;

pub use backend::{Backend, BackendKind, NoopBackend};
pub use backends::autodetect;
pub use lockin::{should_lock_in, LockinVerdict};
pub use orchestrator::{synthesize_from_samples, SynthesisOutcome};
