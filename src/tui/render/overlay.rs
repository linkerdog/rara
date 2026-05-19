use crate::tui::theme::*;
#[path = "overlay_setup.rs"]
mod overlay_setup;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Padding,
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};

use self::overlay_setup::{
    render_api_key_editor_modal, render_base_url_editor_modal, render_model_name_editor_modal,
    render_openai_profile_label_editor_modal, render_permission_picker_modal,
    render_skills_picker_modal,
};
use super::super::command::{
    general_help_text, matching_commands, model_help_text, palette_commands,
    recent_transcript_preview, status_metrics_text, status_prompt_sources_text,
    status_runtime_text, status_workspace_text,
};
use super::super::custom_terminal::Frame;
use super::super::state::{CommandSpec, HelpTab, Overlay, StatusTab, TuiApp};
use super::bottom_pane::desired_bottom_pane_height;
use crate::tui::context_display::render_context_lines;
use crate::tui::status_display::render_status_lines;

pub(super) fn render_overlay(f: &mut Frame, app: &TuiApp, overlay: Overlay) -> Option<(u16, u16)> {
    match overlay {
        Overlay::Help(tab) => {
            let popup = popup_rect(f.area(), 80, 60);
            render_dimmer(f, f.area());
            f.render_widget(Clear, popup);
            render_help_modal(f, app, popup, tab);
            None
        }
        Overlay::CommandPalette => {
            let popup = command_palette_rect(f.area(), app);
            f.render_widget(Clear, popup);
            render_command_palette(f, app, popup);
            None
        }
        Overlay::ModelSearch => {
            let popup = command_palette_rect(f.area(), app);
            f.render_widget(Clear, popup);
            render_model_search(f, app, popup);
            None
        }
        Overlay::Status(tab) => {
            let popup = popup_rect(f.area(), 80, 60);
            render_dimmer(f, f.area());
            f.render_widget(Clear, popup);
            render_status_modal(f, app, popup, tab);
            None
        }
        Overlay::Context => {
            let popup = popup_rect(f.area(), 80, 60);
            render_dimmer(f, f.area());
            f.render_widget(Clear, popup);
            render_context_modal(f, app, popup);
            None
        }
        Overlay::ListPicker(kind) => {
            let popup = if kind == super::super::state::ListPickerKind::Resume {
                popup_rect(f.area(), 96, 80)
            } else {
                bottom_picker_rect(f.area())
            };
            render_dimmer(f, f.area());
            f.render_widget(Clear, popup);
            super::super::list_picker::render_list_picker(f, app, kind, popup);
            None
        }
        Overlay::PermissionPicker => {
            let popup = popup_rect(f.area(), 72, 60);
            render_dimmer(f, f.area());
            f.render_widget(Clear, popup);
            render_permission_picker_modal(f, app, popup);
            None
        }
        Overlay::BaseUrlEditor => {
            let popup = setup_flow_rect(f.area());
            render_dimmer(f, f.area());
            f.render_widget(Clear, popup);
            render_base_url_editor_modal(f, app, popup)
        }
        Overlay::ApiKeyEditor => {
            let popup = setup_flow_rect(f.area());
            render_dimmer(f, f.area());
            f.render_widget(Clear, popup);
            render_api_key_editor_modal(f, app, popup)
        }
        Overlay::ModelNameEditor => {
            let popup = setup_flow_rect(f.area());
            render_dimmer(f, f.area());
            f.render_widget(Clear, popup);
            render_model_name_editor_modal(f, app, popup)
        }
        Overlay::OpenAiProfileLabelEditor => {
            let popup = setup_flow_rect(f.area());
            render_dimmer(f, f.area());
            f.render_widget(Clear, popup);
            render_openai_profile_label_editor_modal(f, app, popup)
        }
        Overlay::SkillsPicker => {
            let popup = setup_flow_rect(f.area());
            render_dimmer(f, f.area());
            f.render_widget(Clear, popup);
            render_skills_picker_modal(f, app, popup);
            None
        }
    }
}

fn render_help_modal(f: &mut Frame, app: &TuiApp, area: Rect, tab: HelpTab) {
    let block = popup_block();
    let inner = block.inner(area);
    f.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(inner);
    let titles = ["General", "Commands", "Runtime"]
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    let selected = match tab {
        HelpTab::General => 0,
        HelpTab::Commands => 1,
        HelpTab::Runtime => 2,
    };
    f.render_widget(
        Tabs::new(titles)
            .select(selected)
            .style(Style::default().fg(TEXT_SECONDARY))
            .highlight_style(help_selected_tab_style()),
        chunks[0],
    );
    match tab {
        HelpTab::General => {
            f.render_widget(
                Paragraph::new(panel_text("general", general_help_text()))
                    .wrap(Wrap { trim: false }),
                chunks[1],
            );
        }
        HelpTab::Commands => {
            let query = app.command_query();
            let items = help_command_items(query)
                .into_iter()
                .map(command_palette_item)
                .collect::<Vec<_>>();
            let mut state = command_palette_list_state(app.command_palette_idx);
            f.render_stateful_widget(
                List::new(items)
                    .highlight_style(command_list_highlight_style())
                    .highlight_symbol("› "),
                chunks[1],
                &mut state,
            );
        }
        HelpTab::Runtime => {
            let inner = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[1]);
            let left = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(8),
                    Constraint::Length(6),
                    Constraint::Min(5),
                ])
                .split(inner[0]);
            let right = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(6), Constraint::Min(8)])
                .split(inner[1]);
            f.render_widget(
                Paragraph::new(panel_text("runtime", &status_runtime_text(app)))
                    .wrap(Wrap { trim: false }),
                left[0],
            );
            f.render_widget(
                Paragraph::new(panel_text("workspace", &status_workspace_text(app)))
                    .wrap(Wrap { trim: false }),
                left[1],
            );
            f.render_widget(
                Paragraph::new(panel_text(
                    "prompt sources",
                    &status_prompt_sources_text(app),
                ))
                .wrap(Wrap { trim: false }),
                left[2],
            );
            f.render_widget(
                Paragraph::new(panel_text("metrics", &status_metrics_text(app)))
                    .wrap(Wrap { trim: false }),
                right[0],
            );
            f.render_widget(
                Paragraph::new(panel_text(
                    "models / recent",
                    &format!(
                        "{}\n\n{}",
                        model_help_text(app),
                        recent_transcript_preview(app, 4)
                    ),
                ))
                .wrap(Wrap { trim: false }),
                right[1],
            );
        }
    }
    f.render_widget(
        Paragraph::new("Esc close  1 general  2 commands  3 runtime  / open slash menu")
            .alignment(Alignment::Center),
        chunks[2],
    );
}

fn render_command_palette(f: &mut Frame, app: &TuiApp, area: Rect) {
    let query = app.command_query();
    let all_items = if query.is_empty() {
        palette_items_for_empty_query(app)
    } else {
        palette_items_for_matches(app, query)
    };

    let mut state = command_palette_list_state(app.command_palette_idx);
    let block = popup_block().title_top(Line::from(Span::styled(
        " Command Palette ",
        Style::default()
            .fg(TEXT_ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Layout: search bar (1) | item list (fill) | footer (1)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // search bar
            Constraint::Fill(1),   // items
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // ── Search bar ───────────────────────────────────────────────
    let search_text = if query.is_empty() {
        Span::styled("  Type to search…", Style::default().fg(TEXT_MUTED))
    } else {
        Span::styled(
            format!("  /{}", query),
            Style::default()
                .fg(BADGE_FG_DARK)
                .add_modifier(Modifier::BOLD),
        )
    };
    f.render_widget(Paragraph::new(Line::from(search_text)), chunks[0]);

    // ── Item list ────────────────────────────────────────────────
    f.render_stateful_widget(
        List::new(all_items)
            .highlight_style(command_list_highlight_style())
            .highlight_symbol("›  "),
        chunks[1],
        &mut state,
    );

    // ── Footer ───────────────────────────────────────────────────
    let count_text = if query.is_empty() {
        format!("{} commands", palette_commands(app, "").len())
    } else {
        format!(
            "{} of {} commands",
            matching_commands(query).len(),
            palette_commands(app, "").len()
        )
    };
    let hints = "↑↓ navigate  ↵ select  Esc close";
    let footer_line = Line::from(vec![
        Span::styled(count_text, Style::default().fg(TEXT_MUTED)),
        Span::styled(format!("    {hints}"), Style::default().fg(TEXT_MUTED)),
    ]);
    f.render_widget(Paragraph::new(footer_line), chunks[2]);
}

fn command_palette_list_state(selected_index: usize) -> ListState {
    let mut state = ListState::default();
    state.select(Some(selected_index));
    state
}

fn palette_items_for_empty_query(app: &TuiApp) -> Vec<ListItem<'static>> {
    palette_commands(app, "")
        .into_iter()
        .map(command_palette_item)
        .collect()
}

fn palette_items_for_matches(_app: &TuiApp, query: &str) -> Vec<ListItem<'static>> {
    matching_commands(query)
        .into_iter()
        .map(command_palette_item)
        .collect()
}

fn help_command_items(query: &str) -> Vec<&'static CommandSpec> {
    matching_commands(query)
}

fn command_palette_item(spec: &CommandSpec) -> ListItem<'static> {
    // Display name with leading slash for consistent width
    let full_name = format!("/{}", spec.name);
    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{full_name:<12}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(spec.summary, Style::default().fg(TEXT_MUTED)),
    ]))
}

fn command_palette_line(spec: &CommandSpec) -> Line<'static> {
    let name_span = Span::styled(
        format!("{:<11}", spec.usage),
        Style::default().add_modifier(Modifier::BOLD),
    );
    let summary_span = Span::styled(spec.summary, Style::default().fg(TEXT_MUTED));
    Line::from(vec![name_span, summary_span])
}

fn panel_text(title: &str, body: &str) -> String {
    format!("{title}\n\n{body}")
}

fn command_list_highlight_style() -> Style {
    Style::default()
        .fg(BADGE_FG_DARK)
        .bg(TEXT_SECONDARY)
        .add_modifier(Modifier::BOLD)
}

fn help_selected_tab_style() -> Style {
    Style::default()
        .fg(BADGE_FG_DARK)
        .bg(TEXT_SECONDARY)
        .add_modifier(Modifier::BOLD)
}

fn render_status_modal(f: &mut Frame, app: &TuiApp, area: Rect, tab: StatusTab) {
    let lines = render_status_lines(app, tab);
    let block = popup_block();
    let inner = block.inner(area);
    f.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .split(inner);
    let titles = status_tab_titles();
    f.render_widget(
        Tabs::new(titles)
            .select(status_tab_index(tab))
            .style(Style::default().fg(TEXT_SECONDARY))
            .highlight_style(help_selected_tab_style()),
        chunks[0],
    );
    f.render_widget(Paragraph::new(lines), chunks[1]);
    f.render_widget(
        Paragraph::new("Esc close  1 overview  2 config  3 context  <-> switch")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center),
        chunks[2],
    );
}

fn status_tab_titles() -> Vec<Line<'static>> {
    ["Overview", "Config", "Context"]
        .into_iter()
        .map(Line::from)
        .collect()
}

fn status_tab_index(tab: StatusTab) -> usize {
    match tab {
        StatusTab::Overview => 0,
        StatusTab::Config => 1,
        StatusTab::Context => 2,
    }
}

fn render_context_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let lines = render_context_lines(app, area.width);
    let block = popup_block();
    let inner = block.inner(area);
    f.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(2)])
        .split(inner);

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.context_scroll, 0)),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new("esc close  j/k ↑↓ scroll").alignment(Alignment::Center),
        chunks[1],
    );
}

/// Bottom-anchored compact popup for list pickers (model, provider, etc.).
/// OpenCode-style: anchored near the input area, not full-screen.
fn bottom_picker_rect(area: Rect) -> Rect {
    // OpenCode-style bottom-anchored compact popup
    let width = area.width.min(76).max(32).clamp(10, area.width);
    let x = area
        .x
        .saturating_add((area.width.saturating_sub(width)) / 2);
    let height = (area.height / 3).min(18).max(12).clamp(6, area.height);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height).saturating_sub(1));
    Rect::new(x, y.max(area.y), width, height)
}

/// Centered popup rect with adaptive sizing.
///
/// Horizontally centered; vertically offset from top by ~1/4 of screen
/// height (opencode style). Width and height are clamped so the popup
/// never exceeds the visible area.
fn popup_rect(area: Rect, max_width: u16, max_height_pct: u16) -> Rect {
    let width = max_width
        .min(area.width.saturating_sub(4))
        .max(20.min(area.width));
    let max_height = (area.height as u32 * max_height_pct as u32 / 100) as u16;
    let height = max_height
        .min(area.height.saturating_sub(4))
        .max(8.min(area.height));
    let top_offset = area.height / 4;
    let x = area
        .x
        .saturating_add((area.width.saturating_sub(width)) / 2);
    let y = (area.y.saturating_add(top_offset))
        .min(area.y.saturating_add(area.height.saturating_sub(height)));
    Rect::new(x, y, width, height)
}

/// Fill the given area with the dimmer background behind popups.
fn render_dimmer(f: &mut Frame, area: Rect) {
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(POPUP_DIMMER_BG)),
        area,
    );
}

/// Styled block for popup content areas: solid panel background, no borders,
/// 1-char horizontal padding (opencode color-block style).
fn popup_block() -> Block<'static> {
    Block::default()
        .style(Style::default().bg(POPUP_BG))
        .padding(Padding::horizontal(1))
}

fn setup_flow_rect(area: Rect) -> Rect {
    let horizontal_margin = if area.width > 140 {
        8
    } else if area.width > 110 {
        4
    } else {
        0
    };
    let vertical_margin = if area.height > 28 {
        2
    } else if area.height > 24 {
        1
    } else {
        0
    };
    let width = area.width.saturating_sub(horizontal_margin * 2).max(24);
    let height = area.height.saturating_sub(vertical_margin * 2).max(8);
    Rect::new(
        area.x.saturating_add(horizontal_margin),
        area.y.saturating_add(vertical_margin),
        width,
        height,
    )
}

fn command_palette_rect(area: Rect, app: &TuiApp) -> Rect {
    // Account for border + search bar (1) + footer (1) = 4 extra rows.
    let query = app.command_query();
    let item_count = if query.is_empty() {
        palette_commands(app, "").len()
    } else {
        matching_commands(query).len()
    };
    let max_visible_rows = area.height.saturating_sub(8).clamp(6, 14) as usize;
    let visible_rows = item_count.clamp(1, max_visible_rows) as u16;
    let height = (visible_rows + 3).min(area.height.saturating_sub(2).max(6));
    let width = area.width;
    let x = area.x;
    // Position above the bottom pane (composer/status) so user input stays visible.
    let bottom_pane_height = desired_bottom_pane_height(app, area.width, area.height);
    let bottom_pane_top = area.y + area.height.saturating_sub(bottom_pane_height);
    let y = bottom_pane_top.saturating_sub(height).max(area.y);

    Rect::new(x, y, width, height)
}

#[cfg(test)]
mod tests {
    use ratatui::widgets::StatefulWidget;
    use ratatui::{buffer::Buffer, layout::Rect};
    use tempfile::tempdir;

    use super::*;
    use crate::config::ConfigManager;
    use crate::tui::command::COMMAND_SPECS;

    #[test]
    fn command_palette_state_scrolls_to_selected_item() {
        let items = (0..20)
            .map(|idx| ListItem::new(format!("item {idx}")))
            .collect::<Vec<_>>();
        let area = Rect::new(0, 0, 20, 5);
        let mut buffer = Buffer::empty(area);
        let mut state = command_palette_list_state(10);

        List::new(items).render(area, &mut buffer, &mut state);

        assert!(state.offset() > 0);
    }

    #[test]
    fn command_palette_line_is_compact_single_row() {
        let spec = &COMMAND_SPECS[0];
        let line = command_palette_line(spec).to_string();

        assert!(line.contains(spec.usage));
        assert!(line.contains(spec.summary));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn help_command_items_are_alphabetical_for_empty_query() {
        let items = help_command_items("");
        let names = items.iter().map(|spec| spec.name).collect::<Vec<_>>();
        let mut sorted = names.clone();
        sorted.sort();

        assert_eq!(items.len(), COMMAND_SPECS.len());
        assert_eq!(names, sorted);
    }

    #[test]
    fn panel_text_prefixes_body_with_lightweight_heading() {
        assert_eq!(
            panel_text("runtime", "provider=codex"),
            "runtime\n\nprovider=codex"
        );
    }

    #[test]
    fn command_palette_rect_anchors_above_frame_bottom() {
        let temp = tempdir().unwrap();
        let app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("build tui app");
        let area = Rect::new(0, 0, 120, 40);

        let popup = command_palette_rect(area, &app);

        // Palette must sit above the bottom pane so the composer input stays visible.
        let bottom_pane_h = desired_bottom_pane_height(&app, area.width, area.height);
        assert!(
            popup.bottom() <= area.y + area.height - bottom_pane_h,
            "palette bottom {} should be above bottom pane top {}",
            popup.bottom(),
            area.y + area.height - bottom_pane_h
        );
    }

    #[test]
    fn command_palette_rect_expands_for_full_empty_query_list() {
        let temp = tempdir().unwrap();
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("build tui app");
        app.bottom_pane.input = "/".into();
        let area = Rect::new(0, 0, 100, 24);

        let popup = command_palette_rect(area, &app);

        assert!(popup.height >= 12);
        assert!(
            popup.width <= area.width,
            "palette should not exceed screen width"
        );
        // Palette must stay above the bottom pane so the composer input stays visible.
        let bottom_pane_h = desired_bottom_pane_height(&app, area.width, area.height);
        assert!(
            popup.bottom() <= area.y + area.height - bottom_pane_h,
            "palette should be entirely above the bottom pane (top at y={})",
            area.y + area.height - bottom_pane_h
        );
    }

    #[test]
    fn setup_flow_rect_is_tall_enough_for_small_terminal_onboarding() {
        let area = Rect::new(0, 0, 100, 24);
        let popup = setup_flow_rect(area);

        assert!(popup.height >= 20);
        assert!(popup.width >= 90);
    }
}

fn render_model_search(f: &mut Frame, app: &TuiApp, area: Rect) {
    let query = app.model_search_query.as_str();
    let presets = app.all_unified_model_presets();
    let filtered: Vec<_> = if query.is_empty() {
        presets.iter().collect()
    } else {
        let q = query.to_ascii_lowercase();
        presets
            .iter()
            .filter(|p| p.model_label.to_ascii_lowercase().contains(&q))
            .collect()
    };

    let mut state = ListState::default();
    state.select(Some(
        app.model_search_idx.min(filtered.len().saturating_sub(1)),
    ));

    let block = popup_block().title_top(Line::from(Span::styled(
        " Model Search ",
        Style::default()
            .fg(TEXT_ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // search input
            Constraint::Min(4),    // model list
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // Search input
    let search_text = if query.is_empty() {
        Span::styled("  Type to filter models…", Style::default().fg(TEXT_MUTED))
    } else {
        Span::styled(
            format!("  {}", query),
            Style::default()
                .fg(BADGE_FG_DARK)
                .add_modifier(Modifier::BOLD),
        )
    };
    f.render_widget(Paragraph::new(Line::from(search_text)), chunks[0]);

    // Model list
    let items: Vec<ListItem> = filtered
        .iter()
        .map(|p| {
            let name = Span::styled(
                format!("{}  ", p.model_label),
                Style::default().add_modifier(Modifier::BOLD),
            );
            let family = Span::styled(
                p.provider_label.as_str(),
                Style::default().fg(TEXT_SECONDARY),
            );
            let window = if let Some(tokens) = p.context_window {
                Span::styled(
                    format!("  · {: >4.0} K", tokens as f64 / 1000.0),
                    Style::default().fg(TEXT_MUTED),
                )
            } else {
                Span::raw("")
            };
            ListItem::new(Line::from(vec![name, family, window]))
        })
        .collect();

    f.render_stateful_widget(
        List::new(items)
            .highlight_style(
                Style::default()
                    .fg(TEXT_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("›  "),
        chunks[1],
        &mut state,
    );

    // Footer
    let footer = Line::from(vec![Span::styled(
        format!(
            "{} models  ↑↓ navigate  ↵ select  Esc close",
            filtered.len()
        ),
        Style::default().fg(TEXT_MUTED),
    )]);
    f.render_widget(Paragraph::new(footer), chunks[2]);
}
