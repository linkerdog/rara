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
        }
    }

    /// Keyboard hint shown at the bottom.
    fn help_text(self) -> &'static str {
        "1-9 jump  Up/Down/jk move  Enter apply  Esc back"
    }

    /// Render the list items for this picker.
    fn render_items(self, app: &TuiApp) -> Vec<ListItem<'static>> {
        let selected = self.idx(app);
        match self {
            Self::Provider => Self::render_provider_items(app, selected),
            _ => {
                // Stub: return placeholder items for pickers not yet migrated.
                (0..self.item_count(app))
                    .map(|_| ListItem::new("(pending migration to ListPicker)"))
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
            .map(|(idx, (family, label, _desc))| {
                let status = if app.selected_provider_family() == *family {
                    " (current)"
                } else {
                    ""
                };
                ListItem::new(vec![ratatui::text::Line::from(format!(
                    "[{}] {}{}",
                    idx + 1,
                    label,
                    status
                ))])
                .style(Self::selected_style(idx, selected))
            })
            .collect()
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

pub fn list_picker_key_event(_kind: ListPickerKind, code: KeyCode) -> AppEvent {
    match code {
        KeyCode::Esc => AppEvent::CloseOverlay,
        KeyCode::Up | KeyCode::Char('k') => AppEvent::MoveListPickerSelection(-1),
        KeyCode::Down | KeyCode::Char('j') => AppEvent::MoveListPickerSelection(1),
        KeyCode::Char('1') => AppEvent::SetListPickerSelection(0),
        KeyCode::Char('2') => AppEvent::SetListPickerSelection(1),
        KeyCode::Char('3') => AppEvent::SetListPickerSelection(2),
        KeyCode::Char('4') => AppEvent::SetListPickerSelection(3),
        KeyCode::Char('5') => AppEvent::SetListPickerSelection(4),
        KeyCode::Char('6') => AppEvent::SetListPickerSelection(5),
        KeyCode::Char('7') => AppEvent::SetListPickerSelection(6),
        KeyCode::Char('8') => AppEvent::SetListPickerSelection(7),
        KeyCode::Char('9') => AppEvent::SetListPickerSelection(8),
        KeyCode::Enter => AppEvent::ApplyOverlaySelection,
        _ => AppEvent::Noop,
    }
}
