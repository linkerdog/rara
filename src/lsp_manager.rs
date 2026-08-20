//! Language Server Protocol (LSP) integration.
//!
//! Implements the asynchronous lifecycle and diagnostics-cache contract in
//! `docs/features/lsp-integration.md`.

mod cache;
mod manager;
mod protocol;
mod runtime;
mod types;

pub use manager::LspManager;
pub use types::{
    Diagnostic, DiagnosticFreshness, DiagnosticSeverity, LspDiagnostics, LspFailure,
    LspFailureKind, LspServerPhase, LspServerStatus, LspStatusSnapshot, ServerKind,
};

#[cfg(test)]
#[path = "lsp_manager/tests.rs"]
mod tests;
