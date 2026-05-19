//! Memory store — facade module.
//!
//! Sections are split into separate files via `include!` to keep each
//! under 800 lines.  All items remain in the same module scope so
//! imports (`use super::*`) work without changes.

include!("memory_types.rs");
include!("memory_store_impl.rs");
include!("memory_records.rs");
include!("memory_store_helpers.rs");

#[cfg(test)]
mod tests {
    include!("memory_store_tests.rs");
}
