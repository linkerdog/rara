mod app_event;
mod auth_mode_picker;
mod clipboard;
mod command;
mod composer_text;
mod context_display;
mod controller;
mod custom_terminal;
mod display_sanitize;
mod event_dispatch;
mod event_loop;
mod event_stream;
mod format;
mod frame_scheduler;
mod highlight;
mod input_control;
#[cfg(test)]
mod input_ownership_tests;
#[cfg(test)]
mod interaction_tests;
mod interaction_text;
mod keymap;
mod line_utils;
mod list_picker;
mod markdown;
mod markdown_render;
mod markdown_stream;
mod message_role;
mod model_search;
mod pane_geometry;
#[cfg(test)]
mod permission_controls_tests;
mod permission_policy;
mod plan_display;
mod provider_flow;
mod queued_input;
mod render;
pub(super) mod runtime;
mod runtime_port;
pub(crate) use self::runtime_port::{
    RuntimeClientPort, RuntimeCommand, RuntimeEventStream, RuntimeMaintenanceCommand,
    RuntimeProjectionEvent,
};
mod selection;
mod session_restore;
pub(crate) mod state;
mod status_display;
mod sub_agent_display;
mod submit;
mod terminal_event;
mod terminal_ui;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
mod text_wrap;
mod theme;
mod tool_text;
mod transcript_text;

#[cfg(test)]
pub(crate) use self::event_dispatch::dispatch_event;
pub use self::event_loop::{StartupResumeTarget, TuiStartupOptions, run_tui};
pub(crate) use self::keymap::map_key_to_event;
pub(crate) use self::session_restore::provider_requires_api_key;
#[cfg(test)]
pub(crate) use self::submit::handle_submit;
pub(crate) use self::terminal_ui::is_ssh_session;
