// Items reserved for planned overlay migration.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, List, ListItem, ListState, Padding, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthChar;

use super::Frame;
use crate::tui::render::bottom_pane::composer::editor_cursor_position;
use crate::tui::state::{PermissionMode, ProviderFamily, TuiApp};
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
    // Keep in sync with PermissionMode enum order (skip Custom).
    let modes: &[(PermissionMode, &str, &str)] = &[
        (
            PermissionMode::Auto,
            "Ask Permissions",
            "Ask before file edits and commands. Only reads are auto-approved. Best for sensitive work.",
        ),
        (
            PermissionMode::AcceptEdits,
            "Auto Accept Edits",
            "Auto-approve file edits and common filesystem commands. Ask for network and destructive operations.",
        ),
        (
            PermissionMode::ReadOnly,
            "Plan Mode",
            "Read and explore only. No file changes permitted. Best for codebase analysis.",
        ),
        (
            PermissionMode::FullAccess,
            "Full Access",
            "Auto-approve everything including network access. For isolated, trusted tasks.",
        ),
    ];

    let title = " Permission Mode ";
    let items = modes
        .iter()
        .enumerate()
        .map(|(idx, (mode, label, desc))| {
            let is_current = app.permission_mode == *mode
                || (app.permission_mode == PermissionMode::Custom
                    && idx == app.permission_picker_idx);
            let style = if idx == app.permission_picker_idx {
                Style::default()
                    .fg(theme_color(ThemeToken::TextAccent))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let current_marker = if is_current && app.permission_mode != PermissionMode::Custom {
                " (current)"
            } else {
                ""
            };
            let mode_label = format!("[{}] {}{}", idx + 1, label, current_marker);
            ListItem::new(vec![
                Line::from(mode_label),
                Line::from(desc.to_string()),
                Line::from(""),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new("Choose how RARA handles file edits, commands, and network access. Press Enter to apply the selected mode.")
            .block(
                Block::default()
                    .style(element_bg())
                    .padding(Padding::horizontal(1))
                    .title(title),
            ),
        chunks[0],
    );
    f.render_widget(List::new(items), chunks[1]);
    f.render_widget(
        Paragraph::new("1-4 jump  Up/Down move  Enter apply  Esc back")
            .alignment(Alignment::Center),
        chunks[2],
    );
}

pub(super) fn render_base_url_editor_modal(
    f: &mut Frame,
    app: &TuiApp,
    area: Rect,
) -> Option<(u16, u16)> {
    let is_openai_compatible = matches!(
        app.selected_provider_family(),
        ProviderFamily::OpenAiCompatible
    );
    let intro_text = if is_openai_compatible {
        "Edit the base URL for the selected OpenAI-compatible endpoint profile.\nLeave it empty to restore that profile's default endpoint."
    } else {
        "Edit the Ollama base URL for this provider.\nLeave it empty to clear the override. Default: http://localhost:11434"
    };
    let intro_height = wrapped_text_height(intro_text, area.width.saturating_sub(2));
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
    let editor = Paragraph::new(app.base_url_input.as_str()).block(
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
        app.base_url_input.as_str(),
        app.base_url_cursor_offset(),
        chunks[1],
    ))
}

pub(super) fn render_skills_picker_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let title = format!(" Skills ({} loaded) ", app.skill_picker_entries.len());
    let intro = "Space toggle enable/disable  Esc close";
    let intro_height = wrapped_text_height(intro, area.width.saturating_sub(2));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(intro_height),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(area);

    f.render_widget(
        Paragraph::new(intro)
            .block(
                Block::default()
                    .style(element_bg())
                    .padding(Padding::horizontal(1))
                    .title(title.as_str()),
            )
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let items: Vec<ListItem> = if app.skill_picker_entries.is_empty() {
        vec![ListItem::new("No skills loaded.")]
    } else {
        app.skill_picker_entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let selected = idx == app.skill_picker_idx;
                let bullet = if selected { "›" } else { " " };
                let check = if entry.enabled { "✔" } else { "✗" };
                let style = if selected {
                    Style::default()
                        .fg(theme_color(ThemeToken::TextAccent))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let scope = &entry.scope;
                let line = format!("{bullet} {check} [{scope}] {title}", title = entry.title,);
                ListItem::new(Line::styled(line, style))
            })
            .collect()
    };

    let mut list_state = ListState::default();
    if !app.skill_picker_entries.is_empty() {
        list_state.select(Some(app.skill_picker_idx));
    }

    f.render_stateful_widget(List::new(items), chunks[1], &mut list_state);

    let footer = if app.skill_picker_entries.is_empty() {
        "Esc close"
    } else {
        "↑↓ move  Space toggle  Esc close"
    };
    f.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center),
        chunks[2],
    );
}

pub(super) fn render_api_key_editor_modal(
    f: &mut Frame,
    app: &TuiApp,
    area: Rect,
) -> Option<(u16, u16)> {
    let (intro_text, title, footer_text) = match app.selected_provider_family() {
        ProviderFamily::OpenAiCompatible => (
            "Paste the API key for the selected OpenAI-compatible endpoint profile.",
            " API Key ",
            "Enter save  Esc back to model picker",
        ),
        ProviderFamily::DeepSeek => (
            "Paste a DeepSeek API key. It is used to load /models and call the selected DeepSeek model.",
            " DeepSeek API Key ",
            "Enter save and load models  Esc back to model picker",
        ),
        _ => (
            "Paste a Codex API key. This is the recommended path for SSH/headless sessions.",
            " Codex API Key ",
            "Enter save and rebuild  Esc back to login guide",
        ),
    };
    let intro_height = wrapped_text_height(intro_text, area.width.saturating_sub(2));
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
    let editor = Paragraph::new(app.api_key_input.as_str()).block(
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
        app.api_key_input.as_str(),
        app.api_key_cursor_offset(),
        chunks[1],
    ))
}

pub(super) fn render_model_name_editor_modal(
    f: &mut Frame,
    app: &TuiApp,
    area: Rect,
) -> Option<(u16, u16)> {
    let intro_text = "Set the model name for the selected OpenAI-compatible endpoint profile.\nExample: gpt-4o-mini, kimi-k2.6, deepseek-chat, or any server-specific model id.";
    let intro_height = wrapped_text_height(intro_text, area.width.saturating_sub(2));
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
    let editor = Paragraph::new(app.model_name_input.as_str()).block(
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
    let intro_height = wrapped_text_height(intro_text.as_str(), area.width.saturating_sub(2));
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
    let editor = Paragraph::new(app.openai_profile_label_input.as_str()).block(
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
