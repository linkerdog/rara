use super::*;

const OPENAI_PROFILE_SETUP_KINDS: [OpenAiEndpointKind; 4] = [
    OpenAiEndpointKind::Custom,
    OpenAiEndpointKind::Kimi,
    OpenAiEndpointKind::KimiCoding,
    OpenAiEndpointKind::Openrouter,
];

pub(in crate::tui) const INPUT_HISTORY_LIMIT: usize = 200;

pub fn openai_profile_setup_kinds() -> &'static [OpenAiEndpointKind] {
    &OPENAI_PROFILE_SETUP_KINDS
}

pub(super) fn terminal_multiplexer_label(
    multiplexer: Option<&rara_terminal_detection::Multiplexer>,
) -> String {
    match multiplexer {
        Some(rara_terminal_detection::Multiplexer::Tmux { version }) => version
            .as_ref()
            .filter(|value| !value.is_empty())
            .map(|version| format!("tmux/{version}"))
            .unwrap_or_else(|| "tmux".to_string()),
        Some(rara_terminal_detection::Multiplexer::Zellij) => "zellij".to_string(),
        None => "-".to_string(),
    }
}

pub(super) fn terminal_remote_label(
    remote: Option<&rara_terminal_detection::RemoteSession>,
) -> &'static str {
    match remote {
        Some(rara_terminal_detection::RemoteSession::Ssh) => "ssh",
        None => "local",
    }
}

pub fn input_requests_command_palette(input: &str) -> bool {
    let trimmed = input.trim_start();
    // Open the palette when the user types a bare '/' or the start of a
    // command name.  Once a space (argument) appears, close it so Enter
    // goes to Submit instead of ApplyOverlaySelection.
    trimmed.starts_with('/') && !trimmed.contains(|c: char| c.is_whitespace())
}

pub(crate) fn contains_structured_planning_output(message: &str) -> bool {
    message.contains("<proposed_plan>")
        || message.contains("<plan>")
        || message.contains("<request_user_input>")
}

pub(super) fn state_db_status_error(prefix: &str, message: impl Into<String>) -> String {
    format!("{prefix}: {}", redact_secrets(message.into()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TextInputTarget {
    Composer,
    ModelSearch,
    BaseUrl,
    ApiKey,
    ModelName,
    OpenAiProfileLabel,
}

pub(super) fn effective_cursor_offset(text: &str, cursor_offset: Option<usize>) -> usize {
    cursor_offset
        .unwrap_or_else(|| text.chars().count())
        .min(text.chars().count())
}

pub(crate) fn char_offset_to_byte_index(text: &str, char_offset: usize) -> usize {
    if char_offset == 0 {
        return 0;
    }

    text.char_indices()
        .nth(char_offset)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

pub(in crate::tui) fn composer_display_char_width(ch: char) -> usize {
    match ch {
        '\t' => 4,
        _ => UnicodeWidthChar::width(ch).unwrap_or(0),
    }
}

pub(super) fn startup_warning_for_config(config: &crate::config::RaraConfig) -> Option<String> {
    if config.provider == "codex" {
        return None;
    }
    if !config.has_api_key() && crate::tui::provider_requires_api_key(&config.provider) {
        Some(format!(
            "Warning: {} is missing an API key. Use /model to configure the current provider.",
            config.provider
        ))
    } else {
        None
    }
}
