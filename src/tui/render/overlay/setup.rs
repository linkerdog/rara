// Items reserved for planned overlay migration.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, List, ListItem, ListState, Padding, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthChar;

use super::Frame;
use crate::tui::composer_text::expand_tabs;
use crate::tui::render::bottom_pane::composer::editor_cursor_position;
use crate::tui::state::{ApiKeyTarget, TuiApp};
use crate::tui::theme::{ThemeToken, theme_color};

fn wrapped_text_height(text: &str, area_width: u16) -> u16 {
    let width = area_width.saturating_sub(2).max(1) as usize;
    let mut rows = 0usize;
    for line in text.split('\n') {
        if line.is_empty() {
            rows += 1;
            continue;
        }
        let mut current_width = 0usize;
        let mut line_rows = 1usize;
        for ch in line.chars() {
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if current_width > 0 && current_width + char_width > width {
                line_rows += 1;
                current_width = 0;
            }
            current_width += char_width;
        }
        rows += line_rows;
    }
    rows as u16 + 2
}

pub(super) fn render_permission_picker_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    use std::sync::atomic::Ordering;

    use crate::tui::permission_policy::PERMISSION_PRESETS;

    let current = app.effective_permission_mode();
    let selected = app.permission_picker_idx.min(PERMISSION_PRESETS.len() - 1);
    let header_text = format!(
        "Current: {}\nmode={} shell={}\nnetwork={} bypass={}\n{}",
        current.label(),
        app.agent_execution_mode_label(),
        app.bash_approval_mode_label(),
        if app.sandbox_network_access.load(Ordering::Relaxed) {
            "on"
        } else {
            "off"
        },
        if app.permission_mode == crate::tui::state::PermissionMode::FullAccess {
            "on"
        } else {
            "off"
        },
        match app.pending_permission_mode {
            Some(mode) => format!("Pending: {} (after current task)", mode.label()),
            None if app.is_busy() => "Changes apply after the current task.".into(),
            None => "Changes apply to this session.".into(),
        }
    );
    let block = Block::default()
        .style(element_bg())
        .padding(Padding::horizontal(1))
        .title(" Permissions ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [header, list, detail, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(header_text), header);
    let items = PERMISSION_PRESETS
        .iter()
        .enumerate()
        .map(|(idx, preset)| {
            let marker = if app.pending_permission_mode == Some(preset.mode) {
                " (pending)"
            } else if current == preset.mode {
                " (current)"
            } else {
                ""
            };
            ListItem::new(format!("[{}] {}{}", idx + 1, preset.title, marker))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(selected));
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(
                Style::default()
                    .fg(theme_color(ThemeToken::TextAccent))
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> "),
        list,
        &mut state,
    );
    f.render_widget(
        Paragraph::new(PERMISSION_PRESETS[selected].description).wrap(Wrap { trim: false }),
        detail,
    );
    f.render_widget(Paragraph::new("1-4 select  Enter apply  Esc close"), footer);
}

pub(super) fn render_skills_picker_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let title = " Skills ";
    let items = if app.skill_picker_entries.is_empty() {
        vec![ListItem::new("No skills loaded.")]
    } else {
        app.skill_picker_entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let availability = if entry.disable_model_invocation {
                    "manual"
                } else {
                    "auto"
                };
                let style = if idx == app.skill_picker_idx {
                    Style::default()
                        .fg(theme_color(ThemeToken::TextAccent))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(format!(
                    "[{}] {} [{}] - {}",
                    availability, entry.name, entry.scope, entry.title
                )))
                .style(style)
            })
            .collect()
    };
    let mut list_state = ListState::default();
    if !app.skill_picker_entries.is_empty() {
        list_state.select(Some(app.skill_picker_idx));
        *list_state.offset_mut() = app.skill_picker_idx;
    }

    let [header, list, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .areas(area);
    f.render_widget(
        Paragraph::new("Read-only: auto = model-invocable; manual = explicit invocation.").block(
            Block::default()
                .style(element_bg())
                .padding(Padding::horizontal(1))
                .title(title),
        ),
        header,
    );
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(
                Style::default()
                    .fg(theme_color(ThemeToken::TextAccent))
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> "),
        list,
        &mut list_state,
    );
    f.render_widget(Paragraph::new("Up/Down navigate  Enter/Esc close"), footer);
}

pub(super) fn render_api_key_editor_modal(
    f: &mut Frame,
    app: &TuiApp,
    target: ApiKeyTarget,
    area: Rect,
) -> Option<(u16, u16)> {
    let registry_intro = format!(
        "Paste an API key for {}. Credentials are saved separately from model configuration.",
        app.registry_credential_target
            .as_deref()
            .unwrap_or("the selected provider")
    );
    let (intro_text, title, footer_text) = match target {
        ApiKeyTarget::Registry => (
            registry_intro.as_str(),
            " Provider API Key ",
            "Enter save  Esc back",
        ),
        ApiKeyTarget::OpenAiCompatible => (
            "Paste the API key for the selected OpenAI-compatible endpoint profile.",
            " API Key ",
            "Enter save  Esc back to model picker",
        ),
        ApiKeyTarget::DeepSeek => (
            "Paste a DeepSeek API key. It is used to load /models and call the selected DeepSeek model.",
            " DeepSeek API Key ",
            "Enter save and load models  Esc back to model picker",
        ),
        ApiKeyTarget::Kimi => (
            "Paste a Moonshot AI API key. It is used to load /models and call the selected Moonshot model.",
            " Moonshot AI API Key ",
            "Enter save and load models  Esc back to model picker",
        ),
        ApiKeyTarget::KimiCoding => (
            "Paste a Kimi Code API key. It is used only with the dedicated Kimi coding endpoint.",
            " Kimi For Coding API Key ",
            "Enter save  Esc back to model picker",
        ),
        ApiKeyTarget::Gemini => (
            "Paste a Gemini API key. It is used to call the selected Gemini model.",
            " Gemini API Key ",
            "Enter save  Esc back to model picker",
        ),
        ApiKeyTarget::Codex => (
            "Paste a Codex API key. This is the recommended path for SSH/headless sessions.",
            " Codex API Key ",
            "Enter save and rebuild  Esc back to login guide",
        ),
    };
    let intro_height = wrapped_text_height(intro_text, area.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(intro_height),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(area);
    let intro = Paragraph::new(intro_text)
        .block(
            Block::default()
                .style(element_bg())
                .padding(Padding::horizontal(1))
                .title(title),
        )
        .wrap(Wrap { trim: false });
    let masked_input = "*".repeat(app.api_key_input.chars().count());
    let editor = Paragraph::new(masked_input.as_str()).block(
        Block::default()
            .style(element_bg())
            .padding(Padding::horizontal(1))
            .title(" Value "),
    );
    let footer = Paragraph::new(footer_text).alignment(Alignment::Center);
    f.render_widget(intro, chunks[0]);
    f.render_widget(editor, chunks[1]);
    f.render_widget(footer, chunks[2]);
    Some(editor_cursor_position(
        masked_input.as_str(),
        app.api_key_cursor_offset(),
        chunks[1],
    ))
}

pub(super) fn render_base_url_editor_modal(
    f: &mut Frame,
    app: &TuiApp,
    area: Rect,
) -> Option<(u16, u16)> {
    let intro_text = "Set the base URL for the selected OpenAI-compatible endpoint profile.\nExample: https://api.openai.com/v1, https://api.deepseek.com/v1, or any provider- or proxy-specific URL.";
    let intro_height = wrapped_text_height(intro_text, area.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(intro_height),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(area);
    let intro = Paragraph::new(intro_text)
        .block(
            Block::default()
                .style(element_bg())
                .padding(Padding::horizontal(1))
                .title(" Base URL "),
        )
        .wrap(Wrap { trim: false });
    let editor = Paragraph::new(expand_tabs(app.base_url_input.as_str())).block(
        Block::default()
            .style(element_bg())
            .padding(Padding::horizontal(1))
            .title(" URL "),
    );
    let footer_text = if app.base_url_input.is_empty() {
        "Type to enter URL  Enter save  Esc cancel"
    } else {
        "Enter save  Esc back to model picker"
    };
    let footer = Paragraph::new(footer_text).alignment(Alignment::Center);
    f.render_widget(intro, chunks[0]);
    f.render_widget(editor, chunks[1]);
    f.render_widget(footer, chunks[2]);
    Some(editor_cursor_position(
        app.base_url_input.as_str(),
        app.base_url_cursor_offset(),
        chunks[1],
    ))
}

pub(super) fn render_model_name_editor_modal(
    f: &mut Frame,
    app: &TuiApp,
    area: Rect,
) -> Option<(u16, u16)> {
    let intro_text = "Set the model name for the selected OpenAI-compatible endpoint profile.\nExample: gpt-4o-mini, kimi-k2.6, deepseek-chat, or any server-specific model id.";
    let intro_height = wrapped_text_height(intro_text, area.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(intro_height),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(area);
    let intro = Paragraph::new(intro_text)
        .block(
            Block::default()
                .style(element_bg())
                .padding(Padding::horizontal(1))
                .title(" Model Name "),
        )
        .wrap(Wrap { trim: false });
    let editor = Paragraph::new(expand_tabs(app.model_name_input.as_str())).block(
        Block::default()
            .style(element_bg())
            .padding(Padding::horizontal(1))
            .title(" Value "),
    );
    let footer =
        Paragraph::new("Enter save  Esc back to model picker").alignment(Alignment::Center);
    f.render_widget(intro, chunks[0]);
    f.render_widget(editor, chunks[1]);
    f.render_widget(footer, chunks[2]);
    Some(editor_cursor_position(
        app.model_name_input.as_str(),
        app.model_name_cursor_offset(),
        chunks[1],
    ))
}

pub(super) fn render_openai_profile_label_editor_modal(
    f: &mut Frame,
    app: &TuiApp,
    area: Rect,
) -> Option<(u16, u16)> {
    let kind = app
        .selected_openai_profile_kind()
        .unwrap_or(crate::config::OpenAiEndpointKind::Custom);
    let intro_text = format!(
        "Create a new {} endpoint profile.\nThis label is only used locally in the picker and status surfaces.",
        kind.label()
    );
    let intro_height = wrapped_text_height(intro_text.as_str(), area.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(intro_height),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(area);
    let intro = Paragraph::new(intro_text)
        .block(
            Block::default()
                .style(element_bg())
                .padding(Padding::horizontal(1))
                .title(" New Endpoint Profile "),
        )
        .wrap(Wrap { trim: false });
    let editor = Paragraph::new(expand_tabs(app.openai_profile_label_input.as_str())).block(
        Block::default()
            .style(element_bg())
            .padding(Padding::horizontal(1))
            .title(" Label "),
    );
    let footer = Paragraph::new("Enter create  Esc back to profiles").alignment(Alignment::Center);
    f.render_widget(intro, chunks[0]);
    f.render_widget(editor, chunks[1]);
    f.render_widget(footer, chunks[2]);
    Some(editor_cursor_position(
        app.openai_profile_label_input.as_str(),
        app.openai_profile_label_cursor_offset(),
        chunks[1],
    ))
}

fn element_bg() -> Style {
    Style::default().bg(theme_color(ThemeToken::UiElementBg))
}

#[cfg(test)]
mod tests {
    use crate::tui::state::{ApiKeyTarget, Overlay, RuntimeSnapshot};
    use crate::tui::testing::TuiHarness;

    #[test]
    fn clipped_setup_editors_keep_overflow_cursors_on_the_rendered_row() {
        for overlay in [
            Overlay::BaseUrlEditor,
            Overlay::ModelNameEditor,
            Overlay::OpenAiProfileLabelEditor,
            Overlay::ApiKeyEditor(ApiKeyTarget::OpenAiCompatible),
        ] {
            let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
            tui.app_mut().open_overlay(overlay);
            let app = tui.app_mut();
            app.base_url_input = "z".repeat(500);
            app.model_name_input = "z".repeat(500);
            app.openai_profile_label_input = "z".repeat(500);
            app.api_key_input = "\u{754c}".repeat(500);
            app.base_url_cursor_offset = Some(0);
            app.model_name_cursor_offset = Some(0);
            app.openai_profile_label_cursor_offset = Some(0);
            app.api_key_cursor_offset = Some(0);
            let (buffer, cursor) = tui.screen_buffer(80, 40);
            let first = cursor.expect("editor cursor");
            let symbol = if matches!(overlay, Overlay::ApiKeyEditor(_)) {
                "*"
            } else {
                "z"
            };
            assert_eq!(buffer[first].symbol(), symbol);
            let visible_width = (first.0..buffer.area.right())
                .take_while(|x| buffer[(*x, first.1)].symbol() == symbol)
                .count();
            assert!(visible_width > 1);
            for offset in [1, visible_width, visible_width + 1, 500] {
                let app = tui.app_mut();
                app.base_url_cursor_offset = Some(offset);
                app.model_name_cursor_offset = Some(offset);
                app.openai_profile_label_cursor_offset = Some(offset);
                app.api_key_cursor_offset = Some(offset);
                let (buffer, cursor) = tui.screen_buffer(80, 40);
                let expected = (first.0 + offset.min(visible_width - 1) as u16, first.1);
                assert_eq!(
                    cursor,
                    Some(expected),
                    "overlay={overlay:?}, offset={offset}"
                );
                assert_eq!(buffer[expected].symbol(), symbol);
            }
        }
    }
}
