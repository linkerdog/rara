mod acp;
mod agent;
mod agents_ext;
mod app_cli;
mod atomic_file;
mod codex_model_catalog;
mod config;
mod context;
mod control_tokens;
mod file_lock;
mod hooks;
mod llm;
mod local_backend;
mod memory_store;
mod oauth;
mod prompt;
mod redaction;
mod runtime_context;
mod runtime_control;
mod runtime_event_bus;
mod sandbox;
mod session;
mod session_transcript;
mod shell_env;
mod skill;
mod state_db;
mod thread_cli;
mod thread_rollout_log;
mod thread_store;
mod todo;
mod tool;
mod tool_result;
mod tools;
mod tui;
mod vectordb;
mod workspace;

use anyhow::Result;

use crate::redaction::redact_secrets;

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
