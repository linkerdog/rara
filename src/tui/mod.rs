mod app_event;
mod auth_mode_picker;
mod command;
mod constants;
mod context_display;
mod custom_terminal;
mod event_dispatch;
mod event_loop;
mod event_stream;
mod format;
mod highlight;
mod insert_history;
mod interaction_text;
mod keymap;
mod layout_utils;
mod line_utils;
mod markdown;
mod markdown_render;
mod markdown_stream;
mod message_role;
mod plan_display;
mod provider_flow;
mod queued_input;
mod render;
mod runtime;
mod session_restore;
mod state;
mod status_display;
mod sub_agent_display;
mod submit;
mod terminal_event;
mod terminal_ui;
#[cfg(test)]
mod tests;
mod theme;
mod tool_text;

#[cfg(test)]
pub(crate) use self::event_dispatch::dispatch_event;
pub use self::event_loop::{StartupResumeTarget, run_tui};
pub(crate) use self::keymap::map_key_to_event;
pub(crate) use self::session_restore::provider_requires_api_key;
#[cfg(test)]
pub(crate) use self::submit::handle_submit;
pub(crate) use self::terminal_ui::is_ssh_session;
