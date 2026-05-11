//! Claude Code plugin loader and command hook executor for RARA.
//!
//! Provides plugin discovery (scan directories, parse plugin.json and
//! hooks/hooks.json) and command hook execution (spawn shell + stdin JSON
//! + exit code). Compatible with the Claude Code plugin directory layout.

pub mod exec;
pub mod loader;
pub mod types;

pub use exec::HookExecutionResult;
pub use exec::HookInput;
pub use exec::execute_command_hook;
pub use loader::discover_plugins;
pub use loader::load_plugin;
pub use loader::registered_hooks_for_plugin;
pub use types::HookEvent;
pub use types::HookHandler;
pub use types::McpConfig;
pub use types::McpServer;
pub use types::Plugin;
pub use types::RegisteredHook;
