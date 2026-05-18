//! Re-export shim: memory file operations live in the `rara-memory` crate.
//!
//! See `crates/rara-memory/src/files.rs` for the implementation and tests.

pub(crate) use rara_memory::files::{
    MemorySearchHit, ensure_memory_dir, global_memory_path, memory_dir, read_memory_file,
    read_summary_for_context, search_memory, session_memory_path, summary_path, update_summary,
    write_memory,
};
