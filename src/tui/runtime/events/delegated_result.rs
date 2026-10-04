use serde::Deserialize;

use crate::tui::display_sanitize::{sanitize_display_line, sanitize_display_text};

#[derive(Deserialize)]
pub(super) struct DelegatedResult {
    pub(super) summary: Option<String>,
    pub(super) request_user_input: Option<DelegatedRequestInput>,
}

#[derive(Deserialize)]
pub(super) struct DelegatedRequestInput {
    pub(super) question: String,
    #[serde(default)]
    pub(super) options: Vec<(String, String)>,
    pub(super) note: Option<String>,
}

/// Decode the delegated tool payload before applying presentation formatting.
pub(super) fn delegated_result(name: &str, content: &str) -> Option<DelegatedResult> {
    if !matches!(name, "explore_agent" | "plan_agent" | "spawn_agent") {
        return None;
    }
    let mut result = match serde_json::from_str::<DelegatedResult>(content) {
        Ok(result) => result,
        Err(error) => {
            log::warn!("failed to decode {name} result for transcript projection: {error}");
            return None;
        }
    };
    if let Some(request) = result.request_user_input.as_mut() {
        request.question = sanitize_display_line(request.question.trim());
        for (label, description) in &mut request.options {
            *label = sanitize_display_line(label.trim());
            *description = sanitize_display_text(description.trim());
        }
        request.note = request
            .note
            .take()
            .map(|note| sanitize_display_text(note.trim()))
            .filter(|note| !note.is_empty());
    }
    Some(result)
}
