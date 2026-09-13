// Grandfathered legacy lints; remove as this crate is cleaned (issue #871).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented
)]

pub mod consolidation;
pub mod dream_prompts;
pub mod files;
pub mod memory_handle;
pub mod memory_model;
