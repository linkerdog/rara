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
    render_api_key_editor_modal, render_auth_mode_picker_modal, render_base_url_editor_modal,
    render_model_name_editor_modal, render_model_picker_modal,
    render_openai_endpoint_kind_picker_modal, render_openai_profile_label_editor_modal,
    render_openai_profile_picker_modal, render_permission_picker_modal,
    render_provider_picker_modal, render_reasoning_effort_picker_modal, render_resume_picker_modal,
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

pub(super) fn render_overlay(
    f: &mut Frame,
    app: &TuiApp,
    overlay: Overlay,
    bottom_pane_area: Rect,
) -> Option<(u16, u16)> {
    match overlay {
        Overlay::Help(tab) => {
            let popup = centered_rect(78, 70, f.area());
            f.render_widget(Clear, popup);
            render_help_modal(f, app, popup, tab);
            None
        }
        Overlay::CommandPalette => {
            let popup = command_palette_rect(f.area(), bottom_pane_area, app);
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
            let popup = centered_rect(72, 70, f.area());
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
                    Constraint::Length(wrapped_text_height(status_runtime_text(app).as_str(), inner[0].width)),
                    Constraint::Min(0),
                ])
                .split(inner[0]);
            f.render_widget(
                Paragraph::new(panel_text("runtime", status_runtime_text(app).as_str()))
                    .block(Block::default().borders(Borders::LEFT))
                    .wrap(Wrap { trim: false }),
                left[0],
            );
            let right = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(wrapped_text_height(
                        status_prompt_sources_text(app).as_str(),
                        inner[1].width,
                    )),
                    Constraint::Length(wrapped_text_height(
                        status_resources_text(app).as_str(),
                        inner[1].width,
                    )),
                    Constraint::Min(0),
                ])
                .split(inner[1]);
            f.render_widget(
                Paragraph::new(panel_text(
                    "prompt-sources",
                    status_prompt_sources_text(app).as_str(),
                ))
                .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
                .wrap(Wrap { trim: false }),
                right[0],
            );
            f.render_widget(
                Paragraph::new(panel_text(
                    "resources",
                    status_resources_text(app).as_str(),
                ))
                .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
                .wrap(Wrap { trim: false }),
                right[1],
            );
        }
    }
    f.render_widget(
        Paragraph::new("1-3 switch tab  Esc close").alignment(Alignment::Center),
        chunks[2],
    );
}

fn render_status_modal(f: &mut Frame, app: &TuiApp, area: Rect, tab: StatusTab) {
    let bottom_pane_area = area;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(area);
    let titles = ["Overview", "Config", "Context"]
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    let selected = match tab {
        StatusTab::Overview => 0,
        StatusTab::Config => 1,
        StatusTab::Context => 2,
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
        StatusTab::Overview => {
            let text = panel_text("status", render_status_lines(app).as_str());
            f.render_widget(
                Paragraph::new(text)
                    .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
                    .wrap(Wrap { trim: false }),
                chunks[1],
            );
        }
        StatusTab::Config => {
            let text = panel_text(
                "config",
                status_workspace_text(app, Some(bottom_pane_area)).as_str(),
            );
            f.render_widget(
                Paragraph::new(text)
                    .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
                    .wrap(Wrap { trim: false }),
                chunks[1],
            );
        }
        StatusTab::Context => {
            f.render_widget(
                Paragraph::new(panel_text("context", render_context_lines(app).as_str()))
                    .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
                    .wrap(Wrap { trim: false }),
                chunks[1],
            );
        }
    }
    f.render_widget(
        Paragraph::new("1-3 switch tab  Esc close").alignment(Alignment::Center),
        chunks[2],
    );
}

fn render_context_modal(f: &mut Frame, app: &TuiApp, area: Rect) {
    let context_lines = render_context_lines(app);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(2)])
        .split(area);
    f.render_widget(
        Paragraph::new(panel_text("Context", context_lines.as_str()))
            .block(Block::default().borders(Borders::ALL).title(" Context "))
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new("Esc close  ↑↓/jk scroll").alignment(Alignment::Center),
        chunks[1],
    );
}

fn render_command_palette(f: &mut Frame, app: &TuiApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(area);
    let title = Block::default()
        .borders(Borders::ALL)
        .title(" Command Palette ");
    let query = app.command_query();
    let mut matching = matching_commands(app, query);
    let mut items = matching
        .iter()
        .map(|spec| command_palette_item(*spec))
        .collect::<Vec<_>>();
    if matching.is_empty() {
        items.push(ListItem::new("No matching commands."));
    }
    let mut state = command_palette_list_state(app.command_palette_idx);

    f.render_widget(
        Paragraph::new("Type to filter commands.  ↑↓/jk move  Enter run  Esc close")
            .block(title),
        chunks[0],
    );
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(command_list_highlight_style())
            .highlight_symbol("› ")
            .block(Block::default().borders(Borders::LEFT | Borders::RIGHT)),
        chunks[1],
        &mut state,
    );
    let hint = if query.is_empty() {
        "Start typing to filter"
    } else {
        ""
    };
    f.render_widget(
        Paragraph::new(hint).alignment(Alignment::Center),
        chunks[2],
    );
}

fn help_selected_tab_style() -> Style {
    Style::default()
        .fg(TEXT_ACCENT)
        .add_modifier(Modifier::BOLD)
}

fn panel_text(label: &str, content: &str) -> String {
    if label.is_empty() || label == "general" || label == "context" {
        content.to_string()
    } else {
        format!("{}\n\n{}", label, content)
    }
}

fn help_command_items(query: &str) -> Vec<&CommandSpec> {
    matching_commands(
        &TuiApp::new(Default::default()).unwrap_or_default(),
        query,
    )
}

fn wrapped_text_height(text: &str, area_width: u16) -> u16 {
    let width = area_width.saturating_sub(2).max(1) as usize;
    let mut rows = 0usize;
    for line in text.split('\n') {
        if line.is_empty() {
            rows += 1;
        } else {
            rows += (line.len() + width - 1) / width;
        }
    }
    rows as u16
}

fn command_palette_rect(
    frame_area: Rect,
    bottom_pane_area: Rect,
    app: &TuiApp,
) -> Rect {
    let prompt_height = bottom_pane_area.height.saturating_sub(2);
    let columns = frame_area.width.min(60);
    let rows = 12u16.clamp(3, frame_area.height.saturating_sub(prompt_height).saturating_sub(2));
    let y = frame_area
        .height
        .saturating_sub(prompt_height)
        .saturating_sub(rows)
        .saturating_sub(1);
    Rect {
        x: 2,
        y,
        width: columns,
        height: rows,
    }
}

fn command_palette_list_state(selected: usize) -> ListState {
    let mut state = ListState::default();
    state.select(Some(selected));
    state
}

fn command_list_highlight_style() -> Style {
    Style::default()
        .fg(TEXT_ACCENT)
        .add_modifier(Modifier::BOLD)
}

fn command_palette_item(spec: &CommandSpec) -> ListItem<'static> {
    ListItem::new(vec![
        Line::from(vec![
            Span::styled(spec.usage, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::raw(spec.summary),
        ]),
        Line::from(vec![Span::styled(
            spec.detail,
            Style::default().fg(TEXT_SECONDARY),
        )]),
    ])
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let width = (area.width * percent_x.min(100) / 100).min(area.width);
    let height = (area.height * percent_y.min(100) / 100).min(area.height);
    let x = area.x.saturating_add((area.width.saturating_sub(width)) / 2);
    let y = area.y.saturating_add((area.height.saturating_sub(height)) / 2);
    Rect { x, y, width, height }
}

fn setup_flow_rect(area: Rect) -> Rect {
    centered_rect(80, 90, area)
}

#[cfg(test)]
mod tests {
    use ratatui::widgets::StatefulWidget;
    use ratatui::{buffer::Buffer, layout::Rect};
    use tempfile::tempdir;

    use super::*;
    use crate::config::ConfigManager;

    #[test]
    fn command_palette_state_scrolls_to_selected_item() {
        let items = (0..20)
            .map(|idx| ListItem::new(format!("item {idx}")))
            .collect::<Vec<_>>();
        let area = Rect::new(0, 0, 20, 5);
        let mut buffer = Buffer::empty(area);
        let mut state = command_palette_list_state(10);
        let list = List::new(items)
            .highlight_style(command_list_highlight_style())
            .highlight_symbol("› ");
        list.render(area, &mut buffer, &mut state);
        assert!(state.selected().is_some());
    }

    #[test]
    fn setup_flow_rect_is_tall_enough_for_small_terminal_onboarding() {
        let area = Rect::new(0, 0, 80, 24);
        let rect = setup_flow_rect(area);
        assert!(rect.height >= 10);
        assert!(rect.width <= 80);
    }

    #[test]
    fn centered_rect_clamps_to_area_bounds() {
        let area = Rect::new(0, 0, 40, 10);
        let rect = centered_rect(80, 80, area);
        assert!(rect.width <= area.width);
        assert!(rect.height <= area.height);
    }
}
