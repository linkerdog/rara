use ratatui::{
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Paragraph},
};

use crate::tui::{
    composer_text::{WrapConfig, wrapped_text},
    custom_terminal::Frame,
    display_sanitize::{sanitize_display_line, sanitize_display_text},
    goal_ui::{GoalDialog, status_label, valid_commands},
    state::TuiApp,
    theme::{ThemeToken, token_fg},
    transcript_text::wrap_lines,
};

pub(super) fn render_goal_dialog(f: &mut Frame, app: &TuiApp, area: Rect) -> Option<(u16, u16)> {
    let dialog = app.goal_ui.dialog.as_ref()?;
    let title = match dialog {
        GoalDialog::Summary => " Goal ",
        GoalDialog::Resume(_) => " Resume paused goal? ",
        GoalDialog::Edit(_) => " Edit goal objective ",
        GoalDialog::Replace { .. } => " Replace unfinished goal? ",
    };
    let block = Block::bordered()
        .title(title)
        .style(token_fg(ThemeToken::TextPrimary));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(inner);
    if matches!(dialog, GoalDialog::Edit(_)) {
        let display = sanitize_display_text(&app.goal_ui.input);
        let cursor = app
            .goal_ui
            .cursor
            .unwrap_or_else(|| app.goal_ui.input.chars().count());
        let prefix: String = app.goal_ui.input.chars().take(cursor).collect();
        let layout = wrapped_text(
            &display,
            WrapConfig {
                width: body.width,
                initial_indent: "",
                subsequent_indent: "",
            },
        );
        let cursor = layout.cursor_position(sanitize_display_text(&prefix).chars().count());
        let scroll = cursor
            .row
            .saturating_sub(body.height.saturating_sub(1) as usize);
        let rows: Vec<Line<'_>> = layout
            .rows()
            .iter()
            .skip(scroll)
            .map(|row| Line::from(row.as_str()))
            .collect();
        f.render_widget(Paragraph::new(rows), body);
        f.render_widget(
            Paragraph::new(wrap_lines(
                &[Line::from(
                    "Enter save · Esc cancel; budget and usage are preserved",
                )],
                footer.width,
            )),
            footer,
        );
        return (body.height > 0).then_some((
            body.x + cursor.column as u16,
            body.y + (cursor.row - scroll) as u16,
        ));
    }
    let mut rows = Vec::new();
    match dialog {
        GoalDialog::Summary => {
            if let Some(goal) = &app.goal {
                rows.push(Line::from(format!("Status: {}", status_label(goal.status))));
                rows.push(Line::from(format!(
                    "Time used: {}s · Turns: {}",
                    goal.time_used_seconds(),
                    goal.turns_completed
                )));
                rows.push(Line::from(format!(
                    "Tokens: {} · Budget: {}",
                    goal.tokens_used,
                    goal.token_budget
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "unlimited".into())
                )));
                if let Some(remaining) = goal.remaining_tokens() {
                    rows.push(Line::from(format!("Remaining: {remaining} tokens")));
                }
                rows.push(Line::from(format!(
                    "Objective: {}",
                    sanitize_display_line(&goal.objective)
                )));
            } else {
                rows.push(Line::from("No goal is set."));
            }
            f.render_widget(
                Paragraph::new(wrap_lines(&[Line::from(valid_commands(app))], footer.width)),
                footer,
            );
        }
        GoalDialog::Resume(_) | GoalDialog::Replace { .. } => {
            if let Some(goal) = &app.goal {
                rows.push(Line::from(format!(
                    "Current: {}",
                    sanitize_display_line(&goal.objective)
                )));
            }
            let choices = if let GoalDialog::Replace {
                objective, budget, ..
            } = dialog
            {
                rows.push(Line::from(format!(
                    "New: {}",
                    sanitize_display_line(objective)
                )));
                rows.push(Line::from(format!(
                    "Budget: {}",
                    budget
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "unlimited".into())
                )));
                ["Replace goal", "Keep current goal"]
            } else {
                ["Resume goal", "Leave paused"]
            };
            rows.push(Line::default());
            for (index, choice) in choices.into_iter().enumerate() {
                rows.push(Line::from(format!(
                    "{} {choice}",
                    if index == app.goal_ui.selected {
                        ">"
                    } else {
                        " "
                    }
                )));
            }
            f.render_widget(
                Paragraph::new(wrap_lines(
                    &[Line::from(
                        "Up/Down select · Enter confirm · Esc leave unchanged",
                    )],
                    footer.width,
                )),
                footer,
            );
        }
        GoalDialog::Edit(_) => unreachable!("editor rendered above"),
    }
    f.render_widget(Paragraph::new(wrap_lines(&rows, body.width)), body);
    None
}
