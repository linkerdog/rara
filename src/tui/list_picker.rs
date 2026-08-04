//! Generic list-picker overlay that renders an interactive scrolling list.
//!
//! Individual pickers (Provider, Model, AuthMode, etc.) use
//! `Overlay::ListPicker(ListPickerKind)` instead of their own `Overlay` variant.
//! The kind drives item count, rendering, and the action taken on Enter.

use crossterm::event::KeyCode;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Padding, Paragraph},
};

use super::app_event::AppEvent;
use super::custom_terminal::Frame;
use super::state::{ListPickerKind, TuiApp};
use super::theme::{ThemeToken, theme_color};
use crate::thread_store::ThreadSummary;

const AUTH_MODE_ITEM_COUNT: usize = 4;

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
            Self::UnifiedModel => app.model_picker_idx,
            Self::NowledgeMem => app.nowledge_mem_picker_idx,
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
            Self::UnifiedModel => app.model_picker_idx = i,
            Self::NowledgeMem => app.nowledge_mem_picker_idx = i,
        }
    }

    /// Number of selectable items.
    pub fn item_count(self, app: &TuiApp) -> usize {
        match self {
            Self::Provider => super::state::PROVIDER_FAMILIES.len(),
            Self::Model => app.current_model_picker_len(),
            Self::OpenAiEndpointKind => super::state::openai_profile_setup_kinds().len(),
            Self::OpenAiProfile => app.selected_openai_profiles().len() + 1,
            Self::Resume => resumable_threads(app).len(),
            Self::AuthMode => AUTH_MODE_ITEM_COUNT,
            Self::ReasoningEffort => app.selected_codex_reasoning_options().len(),
            Self::UnifiedModel => app.all_unified_model_presets().len(),
            Self::NowledgeMem => 3,
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
            Self::UnifiedModel => " All Models ",
            Self::NowledgeMem => " Nowledge Mem ",
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
            Self::UnifiedModel => "Select a model across all providers.",
            Self::NowledgeMem => "Choose the builtin memory connection mode.",
        }
    }

    fn help_text(self) -> &'static str {
        match self {
            Self::UnifiedModel => "Up/Down/jk move  Enter apply  Esc back",
            Self::NowledgeMem => "1-3 jump  Up/Down/jk move  Enter apply  Esc back",
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
            Self::NowledgeMem => Self::render_nowledge_mem_items(app, selected),
        }
    }

    fn selected_style(idx: usize, selected: usize) -> Style {
        if idx == selected {
            Style::default()
                .fg(theme_color(ThemeToken::PickerHighlightFg))
                .bg(theme_color(ThemeToken::PickerHighlightBg))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme_color(ThemeToken::PickerItemFg))
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
                        Style::default().fg(theme_color(ThemeToken::StatusSuccess)),
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
                    Style::default().fg(theme_color(ThemeToken::PickerItemMutedFg)),
                ));
                ListItem::new(vec![name_line, desc_line]).style(Self::selected_style(idx, selected))
            })
            .collect()
    }

    fn render_model_items(app: &TuiApp, selected: usize) -> Vec<ListItem<'static>> {
        use super::state::{PROVIDER_FAMILIES, ProviderFamily};
        let provider_label = PROVIDER_FAMILIES[app.provider_picker_idx].1;
        let family = app.selected_provider_family();
        let mut items: Vec<ListItem<'static>> =
            if matches!(family, ProviderFamily::DeepSeek | ProviderFamily::Kimi) {
                let models = if family == ProviderFamily::DeepSeek {
                    &app.deepseek_model_options
                } else {
                    &app.kimi_model_options
                };
                models
                    .iter()
                    .enumerate()
                    .map(|(idx, model)| {
                        let context = app
                            .model_context_window(family, model)
                            .map(|tokens| format!(" · {:.0}K", tokens as f64 / 1000.0))
                            .unwrap_or_default();
                        ListItem::new(ratatui::text::Line::from(format!(
                            "{} ({}){}",
                            model, provider_label, context,
                        )))
                        .style(Self::selected_style(idx, selected))
                    })
                    .collect()
            } else {
                let presets = super::state::current_model_presets(app.provider_picker_idx);
                presets
                    .iter()
                    .enumerate()
                    .map(|(idx, preset)| {
                        ListItem::new(ratatui::text::Line::from(format!(
                            "{} ({})",
                            preset.1, provider_label,
                        )))
                        .style(Self::selected_style(idx, selected))
                    })
                    .collect()
            };
        if matches!(
            app.selected_provider_family(),
            ProviderFamily::OpenAiCompatible
        ) {
            let base = items.len();
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
        [
            "Browser Login (browser-based OAuth)",
            "Device Code Login (headless/SSH)",
            "API Key",
            "Sign Out",
        ]
        .into_iter()
        .enumerate()
        .map(|(idx, label)| {
            ListItem::new(ratatui::text::Line::from(label))
                .style(Self::selected_style(idx, selected))
        })
        .collect()
    }

    fn render_nowledge_mem_items(app: &TuiApp, selected: usize) -> Vec<ListItem<'static>> {
        let config = &app.config.builtin_plugins.nowledge_mem;
        [
            ("Disabled", "Do not load the builtin Nowledge Mem plugin."),
            ("Local", "Use the local loopback MCP endpoint."),
            (
                "Cloud",
                "Use https://cloud.nowledge.co with env-backed auth.",
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(idx, (label, description))| {
            let current = match idx {
                0 => !config.enabled,
                1 => config.enabled && config.mode == crate::config::NowledgeMemMode::Local,
                2 => config.enabled && config.mode == crate::config::NowledgeMemMode::Cloud,
                _ => false,
            };
            let marker = if current { " (current)" } else { "" };
            ListItem::new(vec![
                ratatui::text::Line::from(format!("[{}] {}{}", idx + 1, label, marker)),
                ratatui::text::Line::from(format!("    {description}")),
            ])
            .style(Self::selected_style(idx, selected))
        })
        .collect()
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
        let summaries = resumable_threads(app);
        if summaries.is_empty() {
            return vec![ListItem::new("No threads available.")];
        }
        let now = current_unix_time_secs();
        summaries
            .iter()
            .enumerate()
            .map(|(idx, summary)| {
                ListItem::new(render_resume_summary_lines(idx, summary, now))
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

pub(crate) fn selected_resumable_thread_id(app: &TuiApp) -> Option<String> {
    resumable_threads(app)
        .get(app.resume_picker_idx)
        .map(|summary| summary.metadata.session_id.clone())
}

pub(crate) fn resumable_threads(app: &TuiApp) -> Vec<&ThreadSummary> {
    app.recent_threads
        .iter()
        .filter(|summary| summary.metadata.session_id != app.snapshot.session_id)
        .filter(|summary| {
            resume_workspace_label(&summary.metadata.cwd)
                == resume_workspace_label(&app.snapshot.cwd)
        })
        .collect()
}

fn render_resume_summary_lines(
    idx: usize,
    summary: &ThreadSummary,
    now: u64,
) -> Vec<Line<'static>> {
    let preview = normalized_resume_preview(summary);
    let metadata = &summary.metadata;
    let workspace = resume_workspace_label(&metadata.cwd);
    let updated = format_resume_age(metadata.updated_at, now);
    let counts = format!(
        "hist={} trans={} compact={}",
        metadata.history_len, metadata.transcript_len, summary.compaction.compaction_count
    );
    let compaction = resume_compaction_detail(summary);

    let title = Line::from(vec![
        Span::raw(format!("[{}] ", idx + 1)),
        Span::styled(preview, Style::default().add_modifier(Modifier::BOLD)),
    ]);
    let metadata = Line::from(format!(
        "     {updated}  {}/{}  mode={} approval={}  cwd={} branch={}  {counts}",
        metadata.provider,
        metadata.model,
        metadata.agent_mode,
        metadata.bash_approval,
        workspace,
        metadata.branch,
    ));

    let mut lines = vec![title, metadata];
    if let Some(compaction) = compaction {
        lines.push(Line::from(format!("     {compaction}")));
    }
    lines
}

fn normalized_resume_preview(summary: &ThreadSummary) -> String {
    let preview = summary.preview.replace('\n', " ");
    let preview = preview.trim();
    if preview.is_empty() {
        "(no transcript preview)".to_string()
    } else {
        preview.to_string()
    }
}

fn resume_workspace_label(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| cwd.to_string())
}

fn current_unix_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn format_resume_age(updated_at: i64, now: u64) -> String {
    if updated_at <= 0 {
        return "updated unknown".to_string();
    }
    let age = now.saturating_sub(updated_at as u64);
    let label = if age < 60 {
        "just now".to_string()
    } else if age < 3600 {
        format!("{}m ago", age / 60)
    } else if age < 86400 {
        format!("{}h ago", age / 3600)
    } else {
        format!("{}d ago", age / 86400)
    };
    format!("updated {label}")
}

fn resume_compaction_detail(summary: &ThreadSummary) -> Option<String> {
    let compaction = &summary.compaction;
    if compaction.compaction_count == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(version) = compaction.boundary_version {
        parts.push(format!("boundary=v{version}"));
    }
    if let Some(count) = compaction.recent_file_count {
        parts.push(format!("recent_files={count}"));
    }
    if let (Some(before), Some(after)) = (compaction.before_tokens, compaction.after_tokens) {
        parts.push(format!("tokens={before}->{after}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!("compact {}", parts.join(" ")))
    }
}

// ---------------------------------------------------------------------------
use crate::tui::render::popup_block;

// ---------------------------------------------------------------------------
// Unified render — one function for all ListPicker variants
// ---------------------------------------------------------------------------

pub fn render_list_picker(f: &mut Frame, app: &TuiApp, kind: ListPickerKind, area: Rect) {
    if kind == ListPickerKind::Resume {
        render_resume_picker(f, app, area);
        return;
    }

    let items = kind.render_items(app);
    let mut state = list_picker_state(kind.idx(app), items.len());

    let block = popup_block();
    let inner = block.inner(area);
    f.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(kind.description()).block(
            Block::default()
                .style(Style::default().bg(theme_color(ThemeToken::UiElementBg)))
                .padding(Padding::horizontal(1))
                .title(kind.title()),
        ),
        chunks[0],
    );
    f.render_stateful_widget(
        List::new(items)
            .block(Block::default().padding(Padding::horizontal(1)))
            .highlight_style(list_picker_highlight_style())
            .highlight_symbol("› "),
        chunks[1],
        &mut state,
    );
    f.render_widget(
        Paragraph::new(kind.help_text()).alignment(Alignment::Center),
        chunks[2],
    );
}

fn render_resume_picker(f: &mut Frame, app: &TuiApp, area: Rect) {
    let items = ListPickerKind::Resume.render_items(app);
    let block = popup_block();
    let inner = block.inner(area);
    f.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(inner);

    let query = if app.resume_search_query.is_empty() {
        "type to filter".to_string()
    } else {
        app.resume_search_query.clone()
    };
    let sort_status = if app.resume_sort_by_created {
        "sort=updated [created]"
    } else {
        "sort=[updated] created"
    };
    let total = resumable_threads(app).len();
    let current = if total == 0 {
        0
    } else {
        app.resume_picker_idx + 1
    };
    let search_line = Line::from(vec![
        Span::styled("Search: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(query),
        Span::raw(format!("  showing {current}/{total}")),
    ]);
    let status_line = Line::from(format!("{sort_status}  left/right sort"));

    f.render_widget(
        Paragraph::new(vec![search_line, status_line]).block(
            Block::default()
                .style(Style::default().bg(theme_color(ThemeToken::UiElementBg)))
                .padding(Padding::horizontal(1))
                .title(ListPickerKind::Resume.title()),
        ),
        chunks[0],
    );

    let mut state = list_picker_state(app.resume_picker_idx, total);
    f.render_stateful_widget(
        List::new(items)
            .block(Block::default().padding(Padding::horizontal(1)))
            .highlight_style(list_picker_highlight_style())
            .highlight_symbol("› "),
        chunks[1],
        &mut state,
    );

    let footer = if app.resume_search_query.is_empty() {
        "type search  tab cwd/all  left/right sort  up/down move  enter resume  esc close"
    } else {
        "type search  backspace edit  esc clear search  enter resume"
    };
    f.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center),
        chunks[2],
    );
}

fn list_picker_state(selected: usize, item_count: usize) -> ListState {
    let mut state = ListState::default();
    if item_count > 0 {
        state.select(Some(selected.min(item_count.saturating_sub(1))));
    }
    state
}

fn list_picker_highlight_style() -> Style {
    Style::default()
        .fg(theme_color(ThemeToken::PickerHighlightFg))
        .bg(theme_color(ThemeToken::PickerHighlightBg))
        .add_modifier(Modifier::BOLD)
}

// ---------------------------------------------------------------------------
// Key handling — shared for all ListPicker variants
// ---------------------------------------------------------------------------

pub fn list_picker_key_event(kind: ListPickerKind, code: KeyCode) -> AppEvent {
    if kind == ListPickerKind::Resume {
        return resume_picker_key_event(code);
    }

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

fn resume_picker_key_event(code: KeyCode) -> AppEvent {
    match code {
        KeyCode::Esc => AppEvent::ClearResumeSearch,
        KeyCode::Up => AppEvent::MoveListPickerSelection(-1),
        KeyCode::Down => AppEvent::MoveListPickerSelection(1),
        KeyCode::Tab => AppEvent::CycleResumeSort,
        KeyCode::BackTab | KeyCode::Left | KeyCode::Right => AppEvent::CycleResumeSort,
        KeyCode::Backspace => AppEvent::Backspace,
        KeyCode::Enter => AppEvent::ApplyOverlaySelection,
        KeyCode::Char(c) if !c.is_control() => AppEvent::InputChar(c),
        _ => AppEvent::Noop,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, widgets::StatefulWidget};
    use tempfile::tempdir;

    use super::*;
    use crate::config::ConfigManager;
    use crate::thread_store::{CompactionRecord, ThreadMetadata};

    #[test]
    fn resume_summary_lines_surface_runtime_location_and_compaction_metadata() {
        let summary = ThreadSummary {
            metadata: ThreadMetadata {
                session_id: "thread-123".to_string(),
                cwd: "/Users/test/projects/rara".to_string(),
                branch: "feature/resume-picker".to_string(),
                provider: "codex".to_string(),
                model: "gpt-5.2".to_string(),
                base_url: None,
                agent_mode: "execute".to_string(),
                bash_approval: "suggestion".to_string(),
                created_at: 0,
                origin_kind: "direct".to_string(),
                forked_from_thread_id: None,
                history_len: 8,
                transcript_len: 5,
                updated_at: 0,
            },
            preview: "User: improve resume picker".to_string(),
            compaction: CompactionRecord {
                compaction_count: 2,
                before_tokens: Some(12_000),
                after_tokens: Some(4_000),
                recent_file_count: Some(3),
                boundary_version: Some(1),
                replaced_start: None,
                replaced_end: None,
                metadata_owner: None,
                recent_files: Vec::new(),
                summary: None,
            },
        };

        let rendered = render_resume_summary_lines(0, &summary, 0)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("[1] User: improve resume picker"));
        assert!(rendered.contains("updated unknown  codex/gpt-5.2"));
        assert!(rendered.contains("mode=execute approval=suggestion"));
        assert!(rendered.contains("cwd=rara branch=feature/resume-picker"));
        assert!(rendered.contains("hist=8 trans=5 compact=2"));
        assert!(rendered.contains("compact boundary=v1 recent_files=3 tokens=12000->4000"));
        assert!(!rendered.contains("compaction runs=2"));
        assert_eq!(rendered.lines().count(), 3);
    }

    #[test]
    fn resume_picker_key_event_treats_printable_keys_as_search_input() {
        assert!(matches!(
            list_picker_key_event(ListPickerKind::Resume, KeyCode::Char('1')),
            AppEvent::InputChar('1')
        ));
        assert!(matches!(
            list_picker_key_event(ListPickerKind::Resume, KeyCode::Char('j')),
            AppEvent::InputChar('j')
        ));
        assert!(matches!(
            list_picker_key_event(ListPickerKind::Resume, KeyCode::Up),
            AppEvent::MoveListPickerSelection(-1)
        ));
        assert!(matches!(
            list_picker_key_event(ListPickerKind::Resume, KeyCode::Tab),
            AppEvent::CycleResumeSort
        ));
    }

    #[test]
    fn list_picker_state_scrolls_to_selected_item() {
        let items = (0..20)
            .map(|idx| ListItem::new(format!("item {idx}")))
            .collect::<Vec<_>>();
        let area = Rect::new(0, 0, 20, 5);
        let mut buffer = Buffer::empty(area);
        let mut state = list_picker_state(15, items.len());

        List::new(items)
            .highlight_style(list_picker_highlight_style())
            .highlight_symbol("› ")
            .render(area, &mut buffer, &mut state);

        assert!(state.offset() > 0);
        assert_eq!(state.selected(), Some(15));
    }

    #[test]
    fn selected_resumable_thread_id_uses_rendered_resume_items() {
        let temp = tempdir().expect("tempdir");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.snapshot.session_id = "current-thread".to_string();
        app.snapshot.cwd = "/tmp/workspaces/rara".to_string();
        app.recent_threads = vec![
            thread_summary("current-thread", "/tmp/workspaces/rara"),
            thread_summary("other-workspace", "/tmp/workspaces/other"),
            thread_summary("resumable-thread", "/var/tmp/rara"),
        ];
        app.resume_picker_idx = 0;

        assert_eq!(
            selected_resumable_thread_id(&app).as_deref(),
            Some("resumable-thread")
        );
        assert_eq!(ListPickerKind::Resume.item_count(&app), 1);
    }

    fn thread_summary(session_id: &str, cwd: &str) -> ThreadSummary {
        ThreadSummary {
            metadata: ThreadMetadata {
                session_id: session_id.to_string(),
                cwd: cwd.to_string(),
                branch: "main".to_string(),
                provider: "codex".to_string(),
                model: "gpt-5.2".to_string(),
                base_url: None,
                agent_mode: "execute".to_string(),
                bash_approval: "suggestion".to_string(),
                created_at: 0,
                origin_kind: "direct".to_string(),
                forked_from_thread_id: None,
                history_len: 1,
                transcript_len: 1,
                updated_at: 0,
            },
            preview: format!("User: {session_id}"),
            compaction: CompactionRecord::default(),
        }
    }
}
