//! Generic list-picker overlay that renders an interactive scrolling list.
//!
//! Individual pickers (Provider, Model, AuthMode, etc.) use
//! `Overlay::ListPicker(ListPickerKind)` instead of their own `Overlay` variant.
//! The kind drives item count, rendering, and the action taken on Enter.

use crossterm::event::KeyCode;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use super::app_event::AppEvent;
use super::custom_terminal::Frame;
use super::state::{ListPickerKind, TuiApp};
use super::theme::*;

impl ListPickerKind {
    /// Return the 0-based index of the highlighted item.
    pub fn idx(self, app: &TuiApp) -> usize {
        match self {
            Self::Provider => app.provider_picker_idx,
            Self::Model => app.model_picker_idx,
            Self::OpenAiEndpointKind => app.openai_endpoint_kind_picker_idx,
            Self::OpenAiProfile => app.openai_profile_picker_idx,
            Self::Resume => app.resume_picker_idx,
            Self::AuthMode => app.auth_mode_idx,
            Self::ReasoningEffort => app.reasoning_effort_picker_idx,
            Self::ApprovalDecision => app.approval_picker_idx,
            Self::UnifiedModel => app.model_picker_idx,
        }
    }

    /// Clamp `i` to valid bounds and store it.
    pub fn set_idx(self, app: &mut TuiApp, i: usize) {
        let i = i.min(self.item_count(app).saturating_sub(1));
        match self {
            Self::Provider => app.provider_picker_idx = i,
            Self::Model => app.model_picker_idx = i,
            Self::OpenAiEndpointKind => app.openai_endpoint_kind_picker_idx = i,
            Self::OpenAiProfile => app.openai_profile_picker_idx = i,
            Self::Resume => app.resume_picker_idx = i,
            Self::AuthMode => app.auth_mode_idx = i,
            Self::ReasoningEffort => app.reasoning_effort_picker_idx = i,
            Self::ApprovalDecision => app.approval_picker_idx = i,
            Self::UnifiedModel => app.model_picker_idx = i,
        }
    }

    /// Number of selectable items.
    pub fn item_count(self, app: &TuiApp) -> usize {
        match self {
            Self::Provider => super::state::PROVIDER_FAMILIES.len(),
            Self::Model => super::state::current_model_presets(app.provider_picker_idx).len(),
            Self::OpenAiEndpointKind => super::state::openai_profile_setup_kinds().len(),
            Self::OpenAiProfile => app.selected_openai_profiles().len() + 1,
            Self::Resume => app.recent_threads.len(),
            Self::AuthMode => super::auth_mode_picker::AUTH_MODE_OPTION_COUNT,
            Self::ReasoningEffort => app.selected_codex_reasoning_options().len(),
            Self::ApprovalDecision => 4,
            Self::UnifiedModel => app.all_unified_model_presets().len(),
        }
    }

    /// Title shown at the top of the overlay.
    pub fn title(self) -> &'static str {
        match self {
            Self::Provider => " Provider ",
            Self::Model => " Model Picker ",
            Self::OpenAiEndpointKind => " Endpoint Kind ",
            Self::OpenAiProfile => " Endpoint Profile ",
            Self::Resume => " Resumable Sessions ",
            Self::AuthMode => " Codex Auth Mode ",
            Self::ReasoningEffort => " Reasoning Level ",
            Self::ApprovalDecision => " Approve ",
            Self::UnifiedModel => " All Models ",
        }
    }

    /// Human-readable description of what the picker does.
    fn description(self) -> &'static str {
        match self {
            Self::Provider => "Select a provider family.",
            Self::Model => "Select a model.",
            Self::OpenAiEndpointKind => "Choose the endpoint type for the new profile.",
            Self::OpenAiProfile => "Select or create an endpoint profile.",
            Self::Resume => "Select a past session to resume.",
            Self::AuthMode => "Choose how Codex authenticates.",
            Self::ReasoningEffort => "Select the reasoning level for the chosen Codex model.",
            Self::ApprovalDecision => {
                "Choose whether to approve Once, match Prefix, Always, or only Suggestion."
            }
            Self::UnifiedModel => "Select a model across all providers.",
        }
    }

    fn help_text(self) -> &'static str {
        match self {
            Self::UnifiedModel => "Up/Down/jk move  Enter apply  Esc back",
            _ => "1-9 jump  Up/Down/jk move  Enter apply  Esc back",
        }
    }

    /// Render the list items for this picker.
    fn render_items(self, app: &TuiApp) -> Vec<ListItem<'static>> {
        let selected = self.idx(app);
        match self {
            Self::Provider => Self::render_provider_items(app, selected),
            Self::Model => Self::render_model_items(app, selected),
            Self::AuthMode => Self::render_auth_mode_items(selected),
            Self::ReasoningEffort => Self::render_reasoning_effort_items(app, selected),
            Self::Resume => Self::render_resume_items(app, selected),
            Self::OpenAiEndpointKind => Self::render_endpoint_kind_items(app, selected),
            Self::OpenAiProfile => Self::render_openai_profile_items(app, selected),
            Self::UnifiedModel => Self::render_unified_model_items(app, selected),
            Self::ApprovalDecision => {
                let labels = [
                    "1. Once (approve this command only)",
                    "2. Prefix (approve matching prefix)",
                    "3. Always (approve all bash commands)",
                    "4. Suggestion only (show, don't execute)",
                ];
                labels
                    .iter()
                    .enumerate()
                    .map(|(i, label)| {
                        ListItem::new(ratatui::text::Line::from(*label))
                            .style(Self::selected_style(i, selected))
                    })
                    .collect()
            }
        }
    }

    fn selected_style(idx: usize, selected: usize) -> Style {
        if idx == selected {
            Style::default()
                .fg(TEXT_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        }
    }

    fn render_provider_items(app: &TuiApp, selected: usize) -> Vec<ListItem<'static>> {
        use super::state::PROVIDER_FAMILIES;
        PROVIDER_FAMILIES
            .iter()
            .enumerate()
            .map(|(idx, (family, label, desc))| {
                let current = if app.selected_provider_family() == *family {
                    " (current)"
                } else {
                    ""
                };
                let connected = app
                    .provider_connection_status
                    .get(family)
                    .cloned()
                    .unwrap_or(false);
                let status_indicator = if connected {
                    ratatui::text::Span::styled(
                        " ● ",
                        Style::default().fg(ratatui::style::Color::Green),
                    )
                } else {
                    ratatui::text::Span::raw("   ")
                };

                let name_line = ratatui::text::Line::from(vec![
                    status_indicator,
                    ratatui::text::Span::styled(
                        format!("[{}] {}{}", idx + 1, label, current),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]);
                let desc_line = ratatui::text::Line::from(ratatui::text::Span::styled(
                    format!("      {}", desc),
                    Style::default().fg(TEXT_MUTED),
                ));
                ListItem::new(vec![name_line, desc_line]).style(Self::selected_style(idx, selected))
            })
            .collect()
    }

    fn render_model_items(app: &TuiApp, selected: usize) -> Vec<ListItem<'static>> {
        use super::state::{PROVIDER_FAMILIES, ProviderFamily, current_model_presets};
        let provider_label = PROVIDER_FAMILIES[app.provider_picker_idx].1;
        let presets = current_model_presets(app.provider_picker_idx);
        let mut items: Vec<ListItem<'static>> = presets
            .iter()
            .enumerate()
            .map(|(idx, preset)| {
                // presets are tuples: (model_id, label, extra)
                ListItem::new(ratatui::text::Line::from(format!(
                    "[{}] {} ({})",
                    idx + 1,
                    preset.1,
                    provider_label,
                )))
                .style(Self::selected_style(idx, selected))
            })
            .collect();
        if matches!(
            app.selected_provider_family(),
            ProviderFamily::OpenAiCompatible
        ) {
            let base = presets.len();
            for (offset, label) in ["Select Profile", "Delete Profile"].iter().enumerate() {
                let idx = base + offset;
                items.push(
                    ListItem::new(ratatui::text::Line::from(format!(
                        "[{}] {}",
                        idx + 1,
                        label
                    )))
                    .style(Self::selected_style(idx, selected)),
                );
            }
        }
        items
    }

    fn render_unified_model_items(app: &TuiApp, selected: usize) -> Vec<ListItem<'static>> {
        app.all_unified_model_presets()
            .iter()
            .enumerate()
            .map(|(idx, preset)| {
                let is_current = app.config.provider == preset.provider_id
                    && app.config.model.as_deref() == Some(&preset.model_id);
                let marker = if is_current { " (current)" } else { "" };
                let status_label = if let Some(status) = &preset.status {
                    format!(" ({})", status)
                } else {
                    String::new()
                };

                ListItem::new(ratatui::text::Line::from(format!(
                    "{}/{}{}{}",
                    preset.provider_label, preset.model_label, status_label, marker
                )))
                .style(Self::selected_style(idx, selected))
            })
            .collect()
    }

    fn render_auth_mode_items(selected: usize) -> Vec<ListItem<'static>> {
        vec![
            ListItem::new(ratatui::text::Line::from(
                "[1] Browser Login (browser-based OAuth)",
            ))
            .style(Self::selected_style(0, selected)),
            ListItem::new(ratatui::text::Line::from(
                "[2] Device Code Login (headless/SSH)",
            ))
            .style(Self::selected_style(1, selected)),
            ListItem::new(ratatui::text::Line::from("[3] API Key"))
                .style(Self::selected_style(2, selected)),
            ListItem::new(ratatui::text::Line::from("[4] Sign Out"))
                .style(Self::selected_style(3, selected)),
        ]
    }

    fn render_reasoning_effort_items(app: &TuiApp, selected: usize) -> Vec<ListItem<'static>> {
        let options = app.selected_codex_reasoning_options();
        options
            .iter()
            .enumerate()
            .map(|(idx, option)| {
                let default_suffix = if option.is_default { " default" } else { "" };
                ListItem::new(vec![
                    ratatui::text::Line::from(format!(
                        "[{}] {}{}",
                        idx + 1,
                        option.label,
                        default_suffix
                    )),
                    ratatui::text::Line::from(option.description.clone()),
                    ratatui::text::Line::from(""),
                ])
                .style(Self::selected_style(idx, selected))
            })
            .collect()
    }

    fn render_resume_items(app: &TuiApp, selected: usize) -> Vec<ListItem<'static>> {
        if app.recent_threads.is_empty() {
            return vec![ListItem::new("No threads available.")];
        }
        app.recent_threads
            .iter()
            .enumerate()
            .map(|(idx, summary)| {
                let preview = if summary.preview.is_empty() {
                    "(no preview)"
                } else {
                    summary.preview.as_str()
                };
                ListItem::new(vec![
                    ratatui::text::Line::from(format!(
                        "[{}] {} / {}  branch={}",
                        idx + 1,
                        summary.metadata.session_id,
                        summary.metadata.provider,
                        summary.metadata.branch,
                    )),
                    ratatui::text::Line::from(format!("     {}", preview)),
                    ratatui::text::Line::from(""),
                ])
                .style(Self::selected_style(idx, selected))
            })
            .collect()
    }

    fn render_endpoint_kind_items(app: &TuiApp, selected: usize) -> Vec<ListItem<'static>> {
        use super::state::openai_profile_setup_kinds;
        openai_profile_setup_kinds()
            .iter()
            .enumerate()
            .map(|(idx, kind)| {
                let label = kind.label();
                let marker = if app.selected_openai_profile_kind() == Some(*kind) {
                    " (current)"
                } else {
                    ""
                };
                ListItem::new(ratatui::text::Line::from(format!(
                    "[{}] {}{}",
                    idx + 1,
                    label,
                    marker
                )))
                .style(Self::selected_style(idx, selected))
            })
            .collect()
    }

    fn render_openai_profile_items(app: &TuiApp, selected: usize) -> Vec<ListItem<'static>> {
        let mut items = vec![
            ListItem::new(ratatui::text::Line::from("[1] + New Profile"))
                .style(Self::selected_style(0, selected)),
        ];
        for (idx, (profile_id, label)) in app.selected_openai_profiles().iter().enumerate() {
            let i = idx + 1;
            let marker = if app.config.active_openai_profile_id() == Some(profile_id.as_str()) {
                " (current)"
            } else {
                ""
            };
            items.push(
                ListItem::new(ratatui::text::Line::from(format!(
                    "[{}] {}{}",
                    i + 1,
                    label,
                    marker
                )))
                .style(Self::selected_style(i, selected)),
            );
        }
        items
    }
}

// ---------------------------------------------------------------------------
// Unified render — one function for all ListPicker variants
// ---------------------------------------------------------------------------

pub fn render_list_picker(f: &mut Frame, app: &TuiApp, kind: ListPickerKind, area: Rect) {
    let items = kind.render_items(app);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);

    f.render_widget(
        Paragraph::new(kind.description())
            .block(Block::default().borders(Borders::ALL).title(kind.title())),
        chunks[0],
    );
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(kind.help_text()).alignment(Alignment::Center),
        chunks[2],
    );
}

// ---------------------------------------------------------------------------
// Key handling — shared for all ListPicker variants
// ---------------------------------------------------------------------------

pub fn list_picker_key_event(kind: ListPickerKind, code: KeyCode) -> AppEvent {
    match code {
        KeyCode::Esc => AppEvent::CloseOverlay,
        KeyCode::Up | KeyCode::Char('k') => AppEvent::MoveListPickerSelection(-1),
        KeyCode::Down | KeyCode::Char('j') => AppEvent::MoveListPickerSelection(1),
        KeyCode::Char('1') if kind != ListPickerKind::UnifiedModel => {
            AppEvent::SetListPickerSelection(0)
        }
        KeyCode::Char('2') if kind != ListPickerKind::UnifiedModel => {
            AppEvent::SetListPickerSelection(1)
        }
        KeyCode::Char('3') if kind != ListPickerKind::UnifiedModel => {
            AppEvent::SetListPickerSelection(2)
        }
        KeyCode::Char('4') if kind != ListPickerKind::UnifiedModel => {
            AppEvent::SetListPickerSelection(3)
        }
        KeyCode::Char('5') if kind != ListPickerKind::UnifiedModel => {
            AppEvent::SetListPickerSelection(4)
        }
        KeyCode::Char('6') if kind != ListPickerKind::UnifiedModel => {
            AppEvent::SetListPickerSelection(5)
        }
        KeyCode::Char('7') if kind != ListPickerKind::UnifiedModel => {
            AppEvent::SetListPickerSelection(6)
        }
        KeyCode::Char('8') if kind != ListPickerKind::UnifiedModel => {
            AppEvent::SetListPickerSelection(7)
        }
        KeyCode::Char('9') if kind != ListPickerKind::UnifiedModel => {
            AppEvent::SetListPickerSelection(8)
        }
        KeyCode::Enter => AppEvent::ApplyOverlaySelection,
        _ => AppEvent::Noop,
    }
}
