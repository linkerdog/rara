use std::ops::Range;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Paragraph, Tabs},
};

use super::{
    command_entry_line, command_list_highlight_style, help_command_items, panel_text, popup_block,
};
use crate::tui::command::{
    general_help_text, model_help_text, recent_transcript_preview, status_metrics_text,
    status_prompt_sources_text, status_runtime_text, status_workspace_text,
};
use crate::tui::context_display::render_context_lines;
use crate::tui::custom_terminal::Frame;
use crate::tui::line_utils::prefix_lines;
use crate::tui::state::{
    HelpTab, Overlay, OverlayNavigation, OverlayScrollLayout, StatusTab, TuiApp,
};
use crate::tui::status_display::render_status_lines;
use crate::tui::theme::{ThemeToken, token_fg};
use crate::tui::transcript_text::{wrap_line, wrap_lines};

struct OverlayBody {
    rows: Vec<Line<'static>>,
    commands: Vec<Range<usize>>,
}

impl OverlayBody {
    fn new(app: &TuiApp, overlay: Overlay, width: u16) -> Option<Self> {
        let lines = match overlay {
            Overlay::Help(HelpTab::Commands) => {
                let mut rows = Vec::new();
                let mut commands = Vec::new();
                let prefix_width = if width >= 3 { 2 } else { 0 };
                for (index, spec) in help_command_items(app.command_query())
                    .into_iter()
                    .enumerate()
                {
                    let selected = index == app.command_palette_idx;
                    let mut entry = prefix_lines(
                        wrap_line(
                            &command_entry_line(app, spec),
                            width.saturating_sub(prefix_width),
                        ),
                        Span::raw(if prefix_width == 0 {
                            ""
                        } else if selected {
                            "› "
                        } else {
                            "  "
                        }),
                        Span::raw(" ".repeat(usize::from(prefix_width))),
                    );
                    if selected {
                        let style = command_list_highlight_style();
                        for line in &mut entry {
                            line.style = line.style.patch(style);
                            for span in &mut line.spans {
                                span.style = span.style.patch(style);
                            }
                        }
                    }
                    commands.push(rows.len()..rows.len() + entry.len());
                    rows.extend(entry);
                }
                return Some(Self { rows, commands });
            }
            Overlay::Help(HelpTab::General) => panel_text("general", general_help_text())
                .lines()
                .map(|line| Line::from(line.to_owned()))
                .collect(),
            Overlay::Help(HelpTab::Runtime) => {
                let sections = [
                    ("runtime", status_runtime_text(app)),
                    ("workspace", status_workspace_text(app)),
                    ("prompt sources", status_prompt_sources_text(app)),
                    ("metrics", status_metrics_text(app)),
                    (
                        "models / recent",
                        format!(
                            "{}\n\n{}",
                            model_help_text(app),
                            recent_transcript_preview(app, 4)
                        ),
                    ),
                ];
                let mut lines = Vec::new();
                for (title, body) in sections {
                    if !lines.is_empty() {
                        lines.push(Line::default());
                    }
                    lines.extend(
                        panel_text(title, &body)
                            .lines()
                            .map(|line| Line::from(line.to_owned())),
                    );
                }
                lines
            }
            Overlay::Status(tab) => render_status_lines(app, tab),
            Overlay::Context => render_context_lines(app, width),
            Overlay::HistorySearch
            | Overlay::Diff
            | Overlay::Goal
            | Overlay::CommandPalette
            | Overlay::ModelSearch
            | Overlay::BaseUrlEditor
            | Overlay::ApiKeyEditor(_)
            | Overlay::ModelNameEditor
            | Overlay::OpenAiProfileLabelEditor
            | Overlay::SkillsPicker
            | Overlay::ListPicker(_)
            | Overlay::PermissionPicker => return None,
        };
        Some(Self {
            rows: wrap_lines(&lines, width),
            commands: Vec::new(),
        })
    }
}

pub(crate) fn navigate_overlay(app: &mut TuiApp, navigation: OverlayNavigation) {
    let Some(overlay) = app.overlay else {
        return;
    };
    let Some(layout) = app.overlay_scroll.layout() else {
        if let (Overlay::Help(HelpTab::Commands), OverlayNavigation::Rows(delta)) =
            (overlay, navigation)
        {
            let count = help_command_items(app.command_query()).len();
            app.command_palette_idx = app
                .command_palette_idx
                .saturating_add_signed(delta as isize)
                .min(count.saturating_sub(1));
        }
        return;
    };
    let Some(body) = OverlayBody::new(app, overlay, layout.width) else {
        return;
    };
    app.overlay_scroll.update_layout(OverlayScrollLayout {
        content_rows: body.rows.len(),
        ..layout
    });
    if !body.commands.is_empty()
        && let OverlayNavigation::Rows(delta) = navigation
    {
        app.command_palette_idx = app
            .command_palette_idx
            .saturating_add_signed(delta as isize)
            .min(body.commands.len() - 1);
        app.overlay_scroll
            .reveal(body.commands[app.command_palette_idx].clone());
        return;
    }
    app.overlay_scroll.navigate(navigation);
    if !body.commands.is_empty() {
        let visible = app.overlay_scroll.visible_range();
        app.command_palette_idx = match navigation {
            OverlayNavigation::Start => 0,
            OverlayNavigation::End => body.commands.len() - 1,
            OverlayNavigation::Rows(_)
            | OverlayNavigation::PageUp
            | OverlayNavigation::PageDown => body
                .commands
                .iter()
                .position(|rows| rows.end > visible.start)
                .unwrap_or(body.commands.len() - 1),
        };
    }
}

pub(super) fn render_modal(f: &mut Frame, app: &mut TuiApp, area: Rect, overlay: Overlay) {
    let block = popup_block();
    let inner = block.inner(area);
    f.render_widget(block, area);
    let hints = if inner.width >= 48 {
        "Esc close  ↑↓/j/k scroll  PgUp/PgDn page  Home/End"
    } else {
        "Esc close  ↑↓/j/k scroll\nPgUp/PgDn page  Home/End"
    };
    let footer = wrap_lines(
        &hints.lines().map(Line::from).collect::<Vec<_>>(),
        inner.width,
    );
    let footer_height = (footer.len() as u16).min(inner.height.saturating_sub(2));
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(footer_height),
    ])
    .split(inner);
    match overlay {
        Overlay::Help(tab) => {
            let index = match tab {
                HelpTab::General => 0,
                HelpTab::Commands => 1,
                HelpTab::Runtime => 2,
            };
            f.render_widget(
                Tabs::new(["1 General", "2 Commands", "3 Runtime"])
                    .padding("", "")
                    .select(index)
                    .style(token_fg(ThemeToken::TextSecondary))
                    .highlight_style(command_list_highlight_style()),
                chunks[0],
            );
        }
        Overlay::Status(tab) => {
            let index = match tab {
                StatusTab::Overview => 0,
                StatusTab::Config => 1,
                StatusTab::Context => 2,
            };
            f.render_widget(
                Tabs::new(["1 Overview", "2 Config", "3 Context"])
                    .padding("", "")
                    .select(index)
                    .style(token_fg(ThemeToken::TextSecondary))
                    .highlight_style(command_list_highlight_style()),
                chunks[0],
            );
        }
        Overlay::Context => f.render_widget(
            Paragraph::new("Context").style(token_fg(ThemeToken::TextSecondary)),
            chunks[0],
        ),
        Overlay::HistorySearch
        | Overlay::Diff
        | Overlay::Goal
        | Overlay::CommandPalette
        | Overlay::ModelSearch
        | Overlay::BaseUrlEditor
        | Overlay::ApiKeyEditor(_)
        | Overlay::ModelNameEditor
        | Overlay::OpenAiProfileLabelEditor
        | Overlay::SkillsPicker
        | Overlay::ListPicker(_)
        | Overlay::PermissionPicker => return,
    }
    let Some(body) = OverlayBody::new(app, overlay, chunks[1].width) else {
        return;
    };
    let first_frame = app.overlay_scroll.layout().is_none();
    app.overlay_scroll.update_layout(OverlayScrollLayout {
        width: chunks[1].width,
        height: chunks[1].height,
        content_rows: body.rows.len(),
    });
    if first_frame && let Some(rows) = body.commands.get(app.command_palette_idx) {
        app.overlay_scroll.reveal(rows.clone());
    }
    let visible = app.overlay_scroll.visible_range();
    f.render_widget(Paragraph::new(body.rows[visible].to_vec()), chunks[1]);
    f.render_widget(
        Paragraph::new(footer).style(token_fg(ThemeToken::TextMuted)),
        chunks[2],
    );
}

#[cfg(test)]
#[path = "read_only_tests.rs"]
mod tests;
