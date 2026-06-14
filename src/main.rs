#![recursion_limit = "512"]
#![allow(unused_imports)]

mod acp;
mod acp_consumer;
mod agent;
mod agents_ext;
mod app_cli;
mod auto_memory;
mod classifier;
mod codex_model_catalog;
mod config;
mod context;
mod control_plane;
mod control_tokens;
mod embedding;
mod google_oauth;
mod hook_registry;
mod hook_runtime;
mod hooks;
mod llm;
mod local_backend;
mod local_model_server;
mod lsp_manager;
mod mcp_connection_manager;
mod mcp_status;
mod mcp_tool_cache;
mod memory_distiller;
mod memory_files;
mod memory_notice;
mod memory_store;
mod oauth;
mod plugin_cli;
mod plugin_middleware;
mod print_consumer;
mod prompt;
mod protocol_sources;
mod runtime_context;
mod runtime_control;
mod runtime_event_bus;
mod sandbox;
mod session;
mod session_context;
mod session_promotion;
mod session_transcript;
mod shell_env;
mod skill;
mod thread_cli;
mod thread_store;
mod todo;
mod tool_result;
mod tools;
mod tui;
mod utils;
mod wire_consumer;
mod workspace;

use anyhow::Result;
use rara_persistence::redaction::redact_secrets;

#[tokio::main]
async fn main() {
    if let Err(err) = main_impl().await {
        eprintln!("{}", redact_secrets(format!("Error: {err}")));
        std::process::exit(1);
    }
}

async fn main_impl() -> Result<()> {
    app_cli::run_cli().await
}
