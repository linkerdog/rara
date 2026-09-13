// Grandfathered legacy lints; remove as this crate is cleaned (issue #871).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented
)]
#![allow(unused_imports)]

pub mod file;
pub mod memory;
pub mod patch;
pub mod planning;
pub mod search;
pub mod tool;
