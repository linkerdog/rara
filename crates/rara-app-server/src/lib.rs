// Grandfathered legacy lints; remove as this crate is cleaned (issue #871).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented
)]

//! App-server protocol contracts shared by transport adapters and the runtime.

pub mod runtime_control;
pub mod stdio_protocol;
