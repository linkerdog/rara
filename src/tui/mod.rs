// Test fixtures use assertions and injected panics; the normal library target
// still enforces these gates in all-targets Clippy runs.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable
    )
)]
#![deny(clippy::print_stdout, clippy::print_stderr)]
#![deny(clippy::disallowed_methods)]

mod app_event;
mod auth_mode_picker;
#[cfg(test)]
mod bidi_display_tests;
mod clipboard;
mod command;
mod composer_text;
mod context_display;
mod controller;
mod custom_terminal;
#[cfg(test)]
mod display_boundary_tests;
mod display_clip;
mod display_sanitize;
mod display_tail;
mod event_dispatch;
mod event_loop;
mod event_stream;
mod format;
mod frame_scheduler;
mod goal_resume;
mod goal_ui;
mod highlight;
mod input_control;
#[cfg(test)]
mod input_ownership_tests;
mod input_text;
#[cfg(test)]
mod interaction_tests;
mod interaction_text;
#[cfg(unix)]
mod job_control;
#[cfg(test)]
mod key_control_tests;
mod keymap;
mod line_utils;
mod list_picker;
mod markdown;
mod markdown_render;
mod markdown_stream;
mod message_role;
mod presentation_revision;
pub(crate) use message_role::MessageRole;
mod model_search;
mod pane_geometry;
#[cfg(test)]
mod paste_input_tests;
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
mod terminal_modes;
mod terminal_ui;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
mod text_wrap;
mod theme;
mod tool_progress;
mod tool_text;
mod transcript_rows;
mod transcript_text;
#[cfg(test)]
mod transcript_work;
#[cfg(test)]
mod unicode_boundary_tests;

#[cfg(test)]
pub(crate) use self::event_dispatch::dispatch_event;
pub use self::event_loop::{StartupResumeTarget, TuiStartupOptions, run_tui};
pub(crate) use self::keymap::map_key_to_event;
pub(crate) use self::session_restore::provider_requires_api_key;
#[cfg(test)]
pub(crate) use self::submit::handle_submit;
pub(crate) use self::terminal_ui::is_ssh_session;
