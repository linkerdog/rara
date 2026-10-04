//! rara-core — shared agent abstractions.
//!
//! Provider-neutral LLM and executable tool contracts.

pub mod llm;
pub mod observation;
pub mod tool;

mod platform;
pub use platform::{PlatformSend, PlatformSync};
