//! Built-in native compressor layers (Layer 1 + Layer 2).
//!
//! DSL-based layers (3 and 5) live in nid-dsl; these layers are tight Rust
//! to avoid any interpreter overhead on the hot path.

pub mod layer1;
pub mod layer2;

pub use layer1::Layer1Generic;
pub use layer2::{detect_format, Layer2Format};
