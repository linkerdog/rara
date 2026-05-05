pub mod specs;
pub mod status;

#[cfg(test)]
mod tests;

pub use self::specs::{COMMAND_SPECS, help_text, recommended_commands};
pub use self::specs::{
    general_help_text, matching_commands, palette_command_by_index, palette_commands,
    parse_local_command,
};
pub use self::status::{
    api_key_status, is_local_provider, model_help_text, recent_transcript_preview,
    status_context_text, status_prompt_sources_text, status_resources_text, status_runtime_text,
    status_workspace_text,
};
