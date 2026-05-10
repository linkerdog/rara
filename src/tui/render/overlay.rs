use crate::tui::theme::*;
#[path = "overlay_setup.rs"]
mod overlay_setup;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};

use self::overlay_setup::{
    render_api_key_editor_modal, render_base_url_editor_modal, render_model_name_editor_modal,
    render_openai_profile_label_editor_modal, render_permission_picker_modal,
    render_skills_picker_modal,
};
use super::super::command::{
    general_help_text, matching_commands, model_help_text, palette_commands,
    recent_transcript_preview, status_prompt_sources_text, status_resources_text,
    status_runtime_text, status_workspace_text,
};
use super::super::custom_terminal::Frame;
use super::super::state::{CommandSpec, HelpTab, Overlay, StatusTab, TuiApp};
use crate::tui::context_display::render_context_lines;
use crate::tui::status_display::render_status_lines;

pub(super) fn render_overlay(f: &mut Frame, app: &TuiApp, overlay: Overlay) -> Option<(u16, u16)> {
    match overlay {
        Overlay::Help(tab) => {
            let popup = centered_rect(78, 70, f.area());
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
        Overlay::Status(tab) => {
            let popup = centered_rect(78, 70, f.area());
            f.render_widget(Clear, popup);
            render_status_modal(f, app, popup, tab);
            None
        }
        Overlay::Context => {
            let popup = centered_rect(78, 70, f.area());
            f.render_widget(Clear, popup);
            render_context_modal(f, app, popup);
            None
        }
        Overlay::ListPicker(kind) => {
            let popup = bottom_picker_rect(f.area());
            f.render_widget(Clear, popup);
            super::super::list_picker::render_list_picker(f, app, kind, popup);
            None
        }
        Overlay::PermissionPicker => {
            let popup = centered_rect(72, 70, f.area());
            f.render_widget(Clear, popup);
            render_permission_picker_modal(f, app, popup);
            None
        }
        Overlay::BaseUrlEditor => {
            let popup = setup_flow_rect(f.area());
            f.render_widget(Clear, popup);
            render_base_url_editor_modal(f, app, popup)
        }
        Overlay::ApiKeyEditor => {
            let popup = setup_flow_rect(f.area());
            f.render_widget(Clear, popup);
            render_api_key_editor_modal(f, app, popup)
        }
        Overlay::ModelNameEditor => {
            let popup = setup_flow_rect(f.area());
            f.render_widget(Clear, popup);
            render_model_name_editor_modal(f, app, popup)
        }
        Overlay::OpenAiProfileLabelEditor => {
            let popup = setup_flow_rect(f.area());
            f.render_widget(Clear, popup);
            render_openai_profile_label_editor_modal(f, app, popup)
        }
        Overlay::SkillsPicker => {
            let popup = setup_flow_rect(f.area());
            f.render_widget(Clear, popup);
            render_skills_picker_modal(f, app, popup);
            None
        }
    }
}

fn render_help_modal(f: &mut Frame, app: &TuiApp, area: Rect, tab: HelpTab) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(area);
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
            .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
            .select(selected)
            .style(Style::default().fg(TEXT_SECONDARY))
            .highlight_style(help_selected_tab_style()),
        chunks[0],
    );
    match tab {
        HelpTab::General => {
            f.render_widget(
                Paragraph::new(panel_text("general", general_help_text()))
                    .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
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
                    .highlight_symbol("› ")
                    .block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
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
                    .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
                    .wrap(Wrap { trim: false }),
                left[0],
            );
            f.render_widget(
                Paragraph::new(panel_text("workspace", &status_workspace_text(app)))
                    .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
                    .wrap(Wrap { trim: false }),
                left[1],
            );
            f.render_widget(
                Paragraph::new(panel_text(
                    "prompt sources",
                    &status_prompt_sources_text(app),
                ))
                .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
                .wrap(Wrap { trim: false }),
                left[2],
            );
            f.render_widget(
                Paragraph::new(panel_text("resources", &status_resources_text(app)))
                    .block(Block::default().borders(Borders::RIGHT))
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
                .block(Block::default().borders(Borders::RIGHT))
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(TEXT_MUTED))
        .title_top(Line::from(Span::styled(
            " Command Palette ",
            Style::default()
                .fg(BADGE_FG_DARK)
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

        assert!(popup.bottom() <= area.y + area.height - 1);
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
    }

    #[test]
    fn setup_flow_rect_is_tall_enough_for_small_terminal_onboarding() {
        let area = Rect::new(0, 0, 100, 24);
        let popup = setup_flow_rect(area);

        assert!(popup.height >= 20);
        assert!(popup.width >= 90);
    }
}
fn render_status_modal(f: &mut Frame, app: &TuiApp, area: Rect, tab: StatusTab) {
    let lines = render_status_lines(app, tab);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .split(area);
    let titles = status_tab_titles();
    f.render_widget(
        Tabs::new(titles)
            .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
            .select(status_tab_index(tab))
            .style(Style::default().fg(TEXT_SECONDARY))
            .highlight_style(help_selected_tab_style()),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
    );
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(2)])
        .split(area);

    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
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

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .flex(Flex::Center)
        .split(vertical[1]);
    horizontal[1]
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
    let max_y = area.y + area.height - 1;
    let y = max_y.saturating_sub(height).max(area.y);

    Rect::new(x, y, width, height)
}
