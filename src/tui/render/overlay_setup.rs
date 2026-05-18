// Items reserved for planned overlay migration.
#![allow(dead_code)]
use std::path::Path;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{
        Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
    },
};
use secrecy::ExposeSecret;
use unicode_width::UnicodeWidthChar;

use super::Frame;
use crate::config::OpenAiEndpointProfile;
use crate::tui::auth_mode_picker::build_auth_mode_picker_view;
use crate::tui::command::api_key_status;
use crate::tui::is_ssh_session;
use crate::tui::render::bottom_pane::composer::editor_cursor_position;
use crate::tui::state::{
    PROVIDER_FAMILIES, PermissionMode, ProviderFamily, TuiApp, current_model_presets,
    openai_profile_setup_kinds,
};
use crate::tui::theme::*;

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

pub(super) fn render_provider_picker_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let items = PROVIDER_FAMILIES
        .iter()
        .enumerate()
        .map(|(idx, (_, label, detail))| {
            let style = if idx == app.provider_picker_idx {
                Style::default()
                    .fg(TEXT_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(vec![
                Line::from(format!("[{}] {}", idx + 1, label)),
                Line::from(format!("  {detail}")),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();
    let intro = "Choose a provider family first, then continue into model selection or setup.";
    let intro_height = wrapped_text_height(intro, area.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(intro_height),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(intro)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Provider Menu "),
            )
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new("number jump  up/down move  enter choose  esc close")
            .alignment(Alignment::Center),
        chunks[2],
    );
}

pub(super) fn render_resume_picker_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let intro = if app.recent_threads.is_empty() {
        "No persisted threads found yet."
    } else {
        "Choose a recent thread to restore its transcript, plan state, and interaction cards."
    };
    let intro_height = wrapped_text_height(intro, area.width);
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
                    .borders(Borders::ALL)
                    .title(" Resume Thread "),
            )
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    let items = if app.recent_threads.is_empty() {
        vec![ListItem::new("No threads available.")]
    } else {
        app.recent_threads
            .iter()
            .enumerate()
            .map(|(idx, session)| {
                let style = if idx == app.resume_picker_idx {
                    Style::default()
                        .fg(TEXT_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let when = if session.metadata.updated_at > 0 {
                    let secs = session.metadata.updated_at;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(secs as u64);
                    let age = now.saturating_sub(secs as u64);
                    if age < 60 {
                        "just now".to_string()
                    } else if age < 3600 {
                        format!("{}m ago", age / 60)
                    } else if age < 86400 {
                        format!("{}h ago", age / 3600)
                    } else {
                        format!("{}d ago", age / 86400)
                    }
                } else {
                    "unknown".to_string()
                };
                let msg_count = session.metadata.history_len;
                let preview = if session.preview.is_empty() {
                    "(empty)".to_string()
                } else {
                    session.preview.replace('\n', " ")
                };
                let workspace = Path::new(&session.metadata.cwd)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .unwrap_or("-");
                ListItem::new(vec![
                    Line::from(format!(
                        "{}  {}  {}  msgs={}",
                        session.metadata.session_id, session.metadata.model, workspace, msg_count,
                    )),
                    Line::from(format!(
                        "  {when}  {}/{}",
                        session.metadata.provider, session.metadata.agent_mode,
                    )),
                    Line::from(format!("  {preview}")),
                ])
                .style(style)
            })
            .collect::<Vec<_>>()
    };
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
    );
    let total = app.recent_threads.len();
    let current = if total > 0 {
        app.resume_picker_idx + 1
    } else {
        0
    };
    let pct = if total > 0 {
        (current * 100) / total
    } else {
        0
    };
    let footer = format!(
        "enter resume   esc exit   ctrl+c exit   ↑/↓ browse   {current} / {total} · {pct}%"
    );
    f.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center),
        chunks[2],
    );
}

fn resume_compaction_label(compaction: &crate::thread_store::CompactionRecord) -> String {
    if compaction.compaction_count == 0 {
        return "compact=0".to_string();
    }
    let mut parts = vec![format!("compact={}", compaction.compaction_count)];
    if let Some(version) = compaction.boundary_version {
        parts.push(format!("boundary=v{version}"));
    }
    if let (Some(start), Some(end)) = (compaction.replaced_start, compaction.replaced_end) {
        parts.push(format!("range={start}..{end}"));
    }
    if let (Some(before), Some(after)) = (compaction.before_tokens, compaction.after_tokens) {
        parts.push(format!("tokens={before}->{after}"));
    }
    parts.join(" ")
}

pub(super) fn render_model_picker_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let provider_label = PROVIDER_FAMILIES[app.provider_picker_idx].1;
    if app.selected_provider_family() == ProviderFamily::OpenAiCompatible {
        render_openai_profile_manager_modal(f, app, area);
        return;
    }
    let items = if app.selected_provider_family() == ProviderFamily::Codex {
        app.codex_model_options
            .iter()
            .enumerate()
            .map(|(idx, preset)| {
                let style = if idx == app.model_picker_idx {
                    Style::default()
                        .fg(TEXT_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let current = if app.config.provider == "codex"
                    && app.config.model.as_deref() == Some(preset.model.as_str())
                {
                    " current"
                } else {
                    ""
                };
                let level = preset
                    .default_reasoning_effort
                    .as_deref()
                    .unwrap_or("default");
                ListItem::new(format!(
                    "[{}] {} ({})  level={}{}",
                    idx + 1,
                    preset.label,
                    preset.model,
                    level,
                    current,
                ))
                .style(style)
            })
            .collect::<Vec<_>>()
    } else if app.selected_provider_family() == ProviderFamily::DeepSeek {
        let mut items = app
            .deepseek_model_options
            .iter()
            .enumerate()
            .map(|(idx, model)| {
                let style = if idx == app.model_picker_idx {
                    Style::default()
                        .fg(TEXT_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let current = if app.config.active_openai_profile_kind()
                    == Some(crate::config::OpenAiEndpointKind::Deepseek)
                    && app.config.model.as_deref() == Some(model.as_str())
                {
                    " current"
                } else {
                    ""
                };
                ListItem::new(format!("[{}] {}{}", idx + 1, model, current)).style(style)
            })
            .collect::<Vec<_>>();
        let action_idx = app.deepseek_api_key_action_idx();
        let style = if app.model_picker_idx == action_idx {
            Style::default()
                .fg(TEXT_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        items.push(
            ListItem::new(format!(
                "[{}] API key  Edit the active DeepSeek API key",
                action_idx + 1
            ))
            .style(style),
        );
        items
    } else {
        let presets = current_model_presets(app.provider_picker_idx);
        presets
            .iter()
            .enumerate()
            .map(|(idx, (label, provider, model))| {
                let style = if idx == app.model_picker_idx {
                    Style::default()
                        .fg(TEXT_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let current = if app.config.provider == *provider
                    && app.config.model.as_deref() == Some(*model)
                {
                    " current"
                } else {
                    ""
                };
                let recommendation = if provider_label == "Candle Local" && idx == 2 {
                    " recommended"
                } else {
                    ""
                };
                ListItem::new(format!(
                    "[{}] {} ({}/{}){}{}",
                    idx + 1,
                    label,
                    provider,
                    model,
                    current,
                    recommendation,
                ))
                .style(style)
            })
            .collect::<Vec<_>>()
    };
    let help = if provider_label == "Codex" && api_key_status(&app.config) == "missing" {
        "Provider: Codex\nAuthentication is required before this preset can be used.\nEnter opens the Codex login guide."
    } else if provider_label == "Codex" {
        &format!(
            "Provider: {provider_label}\nBase URL: {}\nReasoning level: {}\nChoose a model first, then Enter to select the level.",
            app.config
                .base_url
                .as_deref()
                .unwrap_or("https://api.openai.com/v1"),
            app.current_reasoning_effort_label(),
        )
    } else if provider_label == "DeepSeek" {
        &format!(
            "Provider: DeepSeek\nBase URL: {}\nAPI key: {}\nChoose a DeepSeek model. R refreshes /models.",
            app.config
                .base_url
                .as_deref()
                .unwrap_or(crate::config::DEFAULT_DEEPSEEK_BASE_URL),
            api_key_status(&app.config),
        )
    } else {
        &format!(
            "Provider: {provider_label}\nBase URL: {}\nSelect a concrete model preset. Enter applies immediately.",
            app.config
                .base_url
                .as_deref()
                .unwrap_or("http://localhost:11434"),
        )
    };
    let help_height = wrapped_text_height(help, area.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(help_height),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(help).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Model Picker "),
        ),
        chunks[0],
    );
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(if provider_label == "Codex" {
            "1-9 jump  Up/Down move  Enter choose level  Esc close"
        } else if provider_label == "DeepSeek" {
            "1-9 jump  Up/Down move  Enter apply  A api key  R refresh  Esc close"
        } else {
            "1-9 apply directly  Up/Down move  B edit base URL  Enter apply  Esc close"
        })
        .alignment(Alignment::Center),
        chunks[2],
    );
}

fn render_openai_profile_manager_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let active_label = app.config.active_openai_profile_label().unwrap_or("-");
    let active_kind = app
        .config
        .active_openai_profile_kind()
        .unwrap_or(crate::config::OpenAiEndpointKind::Custom)
        .label();
    let help = format!(
        "OpenAI-compatible profiles\nActive: {active_label} ({active_kind})  model={}  key={}",
        app.current_model_label(),
        api_key_status(&app.config),
    );
    let help_height = wrapped_text_height(help.as_str(), area.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(help_height),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(help)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Model Profiles "),
            )
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let profiles = app.openai_model_picker_profiles();
    let rows = profiles
        .iter()
        .map(|profile| openai_profile_table_row(app, profile))
        .collect::<Vec<_>>();
    let header = Row::new(vec![
        Cell::from("Status"),
        Cell::from("Name"),
        Cell::from("Type"),
        Cell::from("Model"),
        Cell::from("Key"),
        Cell::from("Base URL"),
    ])
    .style(Style::default().fg(TEXT_SECONDARY));
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(20),
            Constraint::Length(14),
            Constraint::Length(24),
            Constraint::Length(8),
            Constraint::Min(18),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
    .column_spacing(1)
    .highlight_symbol("› ")
    .row_highlight_style(
        Style::default()
            .fg(TEXT_ACCENT)
            .add_modifier(Modifier::BOLD),
    );
    let selected = if profiles.is_empty() {
        None
    } else {
        Some(app.model_picker_idx.min(profiles.len() - 1))
    };
    let mut table_state = TableState::default().with_selected(selected);
    f.render_stateful_widget(table, chunks[1], &mut table_state);
    f.render_widget(
        Paragraph::new("Space/Enter activate  C create  E edit wizard  D delete active  Esc close")
            .alignment(Alignment::Center),
        chunks[2],
    );
}

fn openai_profile_table_row<'a>(app: &TuiApp, profile: &'a OpenAiEndpointProfile) -> Row<'a> {
    let active = if app.config.active_openai_profile_id() == Some(profile.id.as_str()) {
        "active"
    } else {
        ""
    };
    Row::new(vec![
        Cell::from(active),
        Cell::from(profile.label.as_str()),
        Cell::from(profile.kind.label()),
        Cell::from(profile_model_label(profile)),
        Cell::from(profile_api_key_status(profile)),
        Cell::from(profile_base_url_label(profile)),
    ])
}

fn profile_model_label(profile: &OpenAiEndpointProfile) -> &str {
    profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| profile.kind.default_model())
}

fn profile_base_url_label(profile: &OpenAiEndpointProfile) -> &str {
    profile
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|base_url| !base_url.is_empty())
        .unwrap_or_else(|| profile.kind.default_base_url())
}

fn profile_api_key_status(profile: &OpenAiEndpointProfile) -> &'static str {
    if profile
        .api_key
        .as_ref()
        .is_some_and(|key| !key.expose_secret().trim().is_empty())
    {
        "set"
    } else {
        "missing"
    }
}

pub(super) fn render_openai_profile_picker_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let kind = app
        .selected_openai_profile_kind()
        .unwrap_or(crate::config::OpenAiEndpointKind::Custom);
    let mut items = vec![ListItem::new(vec![
        Line::from("[1] Create new profile"),
        Line::from(format!("  Add another {} endpoint profile.", kind.label())),
        Line::from(""),
    ])];
    items.extend(app.selected_openai_profiles().into_iter().enumerate().map(
        |(idx, (id, label))| {
            let active_suffix = if app.config.active_openai_profile_id() == Some(id.as_str()) {
                " active"
            } else {
                ""
            };
            ListItem::new(vec![
                Line::from(format!("[{}] {}{}", idx + 2, label, active_suffix)),
                Line::from(format!("  id={id}")),
                Line::from(""),
            ])
        },
    ));
    let items = items
        .into_iter()
        .enumerate()
        .map(|(idx, item)| {
            if idx == app.openai_profile_picker_idx {
                item.style(
                    Style::default()
                        .fg(TEXT_ACCENT)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(format!(
            "Choose the active {} endpoint profile, or create a new one.",
            kind.label()
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Endpoint Profiles "),
        ),
        chunks[0],
    );
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new("Up/Down move  Enter choose  Esc back").alignment(Alignment::Center),
        chunks[2],
    );
}

pub(super) fn render_openai_endpoint_kind_picker_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let items = openai_profile_setup_kinds()
        .iter()
        .copied()
        .enumerate()
        .map(|(idx, kind)| {
            let current = if app.config.active_openai_profile_kind() == Some(kind) {
                " current"
            } else {
                ""
            };
            let detail = match kind {
                crate::config::OpenAiEndpointKind::Custom => {
                    "Bring your own endpoint URL, API key, and model id."
                }
                crate::config::OpenAiEndpointKind::Deepseek => {
                    "Use DeepSeek defaults, then fill in the API key."
                }
                crate::config::OpenAiEndpointKind::Kimi => {
                    "Use Kimi defaults, then fill in the API key."
                }
                crate::config::OpenAiEndpointKind::Openrouter => {
                    "Use OpenRouter defaults, then fill in the API key."
                }
            };
            let item = ListItem::new(vec![
                Line::from(format!("[{}] {}{}", idx + 1, kind.label(), current)),
                Line::from(format!("  {detail}")),
                Line::from(""),
            ]);
            if idx == app.openai_endpoint_kind_picker_idx {
                item.style(
                    Style::default()
                        .fg(TEXT_ACCENT)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    let intro = "Choose which OpenAI-compatible endpoint family to configure first.\nThe next steps will walk through the connection fields for that endpoint.";
    let intro_height = wrapped_text_height(intro, area.width);
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
                    .borders(Borders::ALL)
                    .title(" Endpoint Kind "),
            )
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new("1-4 jump  Up/Down move  Enter choose  Esc back")
            .alignment(Alignment::Center),
        chunks[2],
    );
}

pub(super) fn render_reasoning_effort_picker_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let options = app.selected_codex_reasoning_options();
    let title = app
        .selected_codex_model()
        .map(|preset| format!(" Reasoning Level · {} ", preset.label))
        .unwrap_or_else(|| " Reasoning Level ".to_string());
    let items = options
        .iter()
        .enumerate()
        .map(|(idx, option)| {
            let style = if idx == app.reasoning_effort_picker_idx {
                Style::default()
                    .fg(TEXT_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let default_suffix = if option.is_default { " default" } else { "" };
            ListItem::new(vec![
                Line::from(format!("[{}] {}{}", idx + 1, option.label, default_suffix)),
                Line::from(option.description.clone()),
                Line::from(""),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new("Select the reasoning level for the chosen Codex model. Enter persists both the model and the level.")
            .block(Block::default().borders(Borders::ALL).title(title)),
        chunks[0],
    );
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new("1-5 jump  Up/Down move  Enter apply  Esc back")
            .alignment(Alignment::Center),
        chunks[2],
    );
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
                    .fg(TEXT_ACCENT)
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
            .block(Block::default().borders(Borders::ALL).title(title)),
        chunks[0],
    );
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
    );
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
        .block(Block::default().borders(Borders::ALL).title(" Base URL "))
        .wrap(Wrap { trim: false });
    let editor = Paragraph::new(app.base_url_input.as_str())
        .block(Block::default().borders(Borders::ALL).title(" Value "));
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
    let intro_height = wrapped_text_height(intro, area.width);
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
            .block(Block::default().borders(Borders::ALL).title(title.as_str()))
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
                        .fg(TEXT_ACCENT)
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

    f.render_stateful_widget(
        List::new(items).block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
        &mut list_state,
    );

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

pub(super) fn render_auth_mode_picker_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let view = build_auth_mode_picker_view(app, is_ssh_session());
    let intro_height = wrapped_text_height(view.intro.as_str(), area.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(intro_height),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(view.intro)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Codex Login "),
            )
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let body = Paragraph::new(view.lines.join("\n"))
        .block(
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT)
                .title(" Details "),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(body, chunks[1]);

    f.render_widget(
        Paragraph::new(view.footer).alignment(Alignment::Center),
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
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    let editor = Paragraph::new(app.api_key_input.as_str())
        .block(Block::default().borders(Borders::ALL).title(" Value "));
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
        .block(Block::default().borders(Borders::ALL).title(" Model Name "))
        .wrap(Wrap { trim: false });
    let editor = Paragraph::new(app.model_name_input.as_str())
        .block(Block::default().borders(Borders::ALL).title(" Value "));
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
                .borders(Borders::ALL)
                .title(" New Endpoint Profile "),
        )
        .wrap(Wrap { trim: false });
    let editor = Paragraph::new(app.openai_profile_label_input.as_str())
        .block(Block::default().borders(Borders::ALL).title(" Label "));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_compaction_label_includes_boundary_range_and_tokens() {
        let label = resume_compaction_label(&crate::thread_store::CompactionRecord {
            compaction_count: 2,
            before_tokens: Some(12_000),
            after_tokens: Some(4_000),
            recent_file_count: Some(3),
            boundary_version: Some(1),
            replaced_start: Some(0),
            replaced_end: Some(8),
            metadata_owner: Some("runtime.compaction".to_string()),
            recent_files: vec![],
            summary: Some("summary".to_string()),
        });

        assert_eq!(label, "compact=2 boundary=v1 range=0..8 tokens=12000->4000");
    }
}
