use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::line_utils::prefix_lines;
use super::state::TuiApp;
use crate::tui::theme::*;

#[derive(Clone, Copy)]
enum PlanStepKind {
    Completed,
    InProgress,
    Pending,
}

impl PlanStepKind {
    fn from_status(status: &str) -> Self {
        match status {
            "completed" => Self::Completed,
            "in_progress" => Self::InProgress,
            _ => Self::Pending,
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Completed => "✓ ",
            Self::InProgress => "» ",
            Self::Pending => "· ",
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Completed => Style::default().add_modifier(Modifier::DIM),
            Self::InProgress => Style::default().fg(TEXT_ACCENT),
            Self::Pending => Style::default().fg(TEXT_MUTED),
        }
    }
}

pub(crate) fn status_plan_text(app: &TuiApp) -> String {
    if !should_show_updated_plan(app) {
        return "No active structured plan.".to_string();
    }
    updated_plan_text(
        app.snapshot.plan_steps.as_slice(),
        app.snapshot.plan_explanation.as_deref(),
    )
}

pub(crate) fn should_show_updated_plan(app: &TuiApp) -> bool {
    if app.snapshot.plan_steps.is_empty() {
        return false;
    }

    matches!(
        app.agent_execution_mode,
        crate::agent::AgentExecutionMode::Plan
    ) || app.has_pending_plan_approval()
}

pub(crate) fn updated_plan_text(steps: &[(String, String)], explanation: Option<&str>) -> String {
    if steps.is_empty() {
        return "No structured plan captured yet.".to_string();
    }

    let mut lines = vec!["Updated Plan".to_string()];

    if let Some(note) = explanation.map(str::trim).filter(|note| !note.is_empty()) {
        lines.push(format!("  note: {note}"));
    }

    for (status, step) in steps {
        let kind = PlanStepKind::from_status(status);
        lines.push(format!("  {}{}", kind.marker(), step));
    }

    lines.join("\n")
}

pub(crate) fn updated_plan_lines(
    steps: &[(String, String)],
    explanation: Option<&str>,
) -> Vec<Line<'static>> {
    if steps.is_empty() {
        return vec![Line::from("No structured plan captured yet.")];
    }

    let mut lines = vec![Line::from(vec![
        Span::styled("• ", Style::default().add_modifier(Modifier::DIM)),
        Span::styled(
            "Updated Plan",
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ])];

    let mut detail_lines = Vec::new();

    if let Some(note) = explanation.map(str::trim).filter(|note| !note.is_empty()) {
        detail_lines.push(Line::from(Span::styled(
            note.to_string(),
            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
        )));
    }

    for (status, step) in steps {
        let kind = PlanStepKind::from_status(status);
        detail_lines.push(Line::from(vec![
            Span::styled(kind.marker(), kind.style()),
            Span::styled(step.to_string(), kind.style()),
        ]));
    }

    lines.extend(prefix_lines(
        detail_lines,
        Span::styled("  └ ", Style::default().add_modifier(Modifier::DIM)),
        Span::raw("    "),
    ));

    lines
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{should_show_updated_plan, updated_plan_lines, updated_plan_text};
    use crate::config::ConfigManager;
    use crate::tui::state::TuiApp;

    #[test]
    fn updated_plan_text_formats_note_and_checklist() {
        let rendered = updated_plan_text(
            &[
                ("completed".into(), "Inspect the current workflow".into()),
                (
                    "in_progress".into(),
                    "Generalize instruction discovery".into(),
                ),
                ("pending".into(), "Validate restore behavior".into()),
            ],
            Some("Keep the explanation short and decision-complete."),
        );

        assert!(rendered.contains("Updated Plan"));
        assert!(rendered.contains("note: Keep the explanation short and decision-complete."));
        assert!(rendered.contains("✓ Inspect the current workflow"));
        assert!(rendered.contains("» Generalize instruction discovery"));
        assert!(rendered.contains("· Validate restore behavior"));
    }

    #[test]
    fn updated_plan_lines_render_title_and_steps() {
        let rendered = updated_plan_lines(
            &[(
                "pending".into(),
                "Capture the next implementation step".into(),
            )],
            Some("Do not claim execution in planning mode."),
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(rendered.contains("Updated Plan"));
        assert!(rendered.contains("Do not claim execution in planning mode."));
        assert!(rendered.contains("· Capture the next implementation step"));
    }

    #[test]
    fn active_plan_visibility_requires_plan_mode_or_approval() {
        let temp = tempdir().expect("tempdir");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("build tui app");
        app.snapshot.plan_steps = vec![("pending".into(), "Inspect core modules".into())];

        assert!(!should_show_updated_plan(&app));

        app.set_pending_plan_approval(true);
        assert!(should_show_updated_plan(&app));
    }
}
