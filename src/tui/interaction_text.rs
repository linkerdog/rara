use super::state::{ActivePendingInteractionKind, TuiApp};

pub fn pending_interaction_section_title(kind: ActivePendingInteractionKind) -> &'static str {
    match kind {
        ActivePendingInteractionKind::PlanApproval => " Plan Approval ",
        ActivePendingInteractionKind::ShellApproval => " Shell Approval ",
        ActivePendingInteractionKind::PlanningQuestion => " Planning Question ",
        ActivePendingInteractionKind::ExplorationQuestion => " Exploration Question ",
        ActivePendingInteractionKind::SubAgentQuestion => " Sub-agent Question ",
        ActivePendingInteractionKind::RequestInput => " Request Input ",
    }
}

pub fn pending_interaction_card_title(kind: ActivePendingInteractionKind) -> &'static str {
    match kind {
        ActivePendingInteractionKind::PlanApproval => "Awaiting Approval",
        ActivePendingInteractionKind::ShellApproval => "Shell Approval",
        ActivePendingInteractionKind::PlanningQuestion => "Planning Question",
        ActivePendingInteractionKind::ExplorationQuestion => "Exploration Question",
        ActivePendingInteractionKind::SubAgentQuestion => "Sub-agent Question",
        ActivePendingInteractionKind::RequestInput => "Request Input",
    }
}

pub fn pending_interaction_hint_text(kind: ActivePendingInteractionKind) -> &'static str {
    match kind {
        ActivePendingInteractionKind::PlanApproval => {
            "plan ready  1 start implementation  2 continue planning"
        }
        ActivePendingInteractionKind::ShellApproval => {
            "approval required  up/down select  enter apply  1-4 shortcut"
        }
        ActivePendingInteractionKind::PlanningQuestion
        | ActivePendingInteractionKind::ExplorationQuestion
        | ActivePendingInteractionKind::SubAgentQuestion
        | ActivePendingInteractionKind::RequestInput => "type reply  1/2/3 shortcut",
    }
}

pub fn pending_interaction_shortcut_text(
    kind: ActivePendingInteractionKind,
) -> Option<&'static str> {
    match kind {
        ActivePendingInteractionKind::PlanApproval => {
            Some("1 start implementation  2 continue planning")
        }
        ActivePendingInteractionKind::ShellApproval => None,
        ActivePendingInteractionKind::PlanningQuestion
        | ActivePendingInteractionKind::ExplorationQuestion
        | ActivePendingInteractionKind::SubAgentQuestion
        | ActivePendingInteractionKind::RequestInput => Some("1/2/3 shortcut"),
    }
}

pub fn status_plan_approval_text(app: &TuiApp) -> String {
    let _ = app;
    "Plan ready for implementation.\n\n1. Start implementation now\n2. Continue planning and refine the plan".to_string()
}

pub fn pending_interaction_detail_text(app: &TuiApp, kind: ActivePendingInteractionKind) -> String {
    match kind {
        ActivePendingInteractionKind::PlanApproval => status_plan_approval_text(app),
        ActivePendingInteractionKind::ShellApproval => status_command_approval_text(app),
        ActivePendingInteractionKind::PlanningQuestion
        | ActivePendingInteractionKind::ExplorationQuestion
        | ActivePendingInteractionKind::SubAgentQuestion
        | ActivePendingInteractionKind::RequestInput => status_request_user_input_text(app),
    }
}

/// Reserved for status-panel pending interaction summaries tracked by docs/todo.md.
#[allow(dead_code)]
pub fn status_active_pending_interaction_text(app: &TuiApp) -> Option<(&'static str, String)> {
    let pending = app.active_pending_interaction()?;
    let title = pending_interaction_section_title(pending.kind);
    let text = pending_interaction_detail_text(app, pending.kind);
    Some((title, text))
}

pub fn status_planning_suggestion_text(app: &TuiApp) -> String {
    let _ = app.bottom_pane.pending_planning_suggestion.as_deref();
    "suggestion:\nThis looks like a non-trivial task. Enter planning mode first so RARA can analyze the repository, refine the approach, and only stop once a concrete plan is ready.\n\noptions:\n1. Enter planning mode\n2. Continue in execute mode".to_string()
}

pub fn status_request_user_input_text(app: &TuiApp) -> String {
    let Some(interaction) = app.pending_request_input() else {
        return "No pending structured question.".to_string();
    };

    let options_text = if interaction.options.is_empty() {
        "No predefined options.".to_string()
    } else {
        interaction
            .options
            .iter()
            .enumerate()
            .map(|(idx, (label, description))| {
                if description.is_empty() {
                    format!("{}. {}", idx + 1, label)
                } else {
                    format!("{}. {} — {}", idx + 1, label, description)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let source_block = interaction
        .source
        .as_deref()
        .map(|source| format!("from: {source}\n\n"))
        .unwrap_or_default();

    if let Some(note) = interaction.note.as_deref() {
        format!(
            "{}{}\n\n{}\n\nnote: {}",
            source_block, interaction.title, options_text, note
        )
    } else {
        format!("{}{}\n\n{}", source_block, interaction.title, options_text)
    }
}

pub fn status_command_approval_text(app: &TuiApp) -> String {
    shell_approval_text_lines(app, None).join("\n")
}

pub fn shell_approval_text_lines(app: &TuiApp, selected: Option<usize>) -> Vec<String> {
    let Some(interaction) = app.pending_command_approval() else {
        return vec!["No pending shell approval.".to_string()];
    };
    let Some(approval) = interaction.approval.as_ref() else {
        return vec!["No pending shell approval.".to_string()];
    };

    let cwd = approval
        .payload
        .cwd
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(".");
    let env_count = approval.payload.env.len();
    let prefix = approval
        .payload
        .approval_prefix()
        .unwrap_or_else(|| approval.command.clone());

    let network = if approval.allow_net {
        "enabled by sandbox config or command request"
    } else {
        "disabled unless already allowed by the sandbox"
    };

    let option_line = |index: usize, text: String| match selected {
        Some(selected_idx) if selected_idx == index => format!("> {text}"),
        _ => format!("  {text}"),
    };

    vec![
        "Review this shell command before RARA runs it.".to_string(),
        String::new(),
        "Command:".to_string(),
        format!("  {}", approval.command),
        String::new(),
        "Working directory:".to_string(),
        format!("  {cwd}"),
        String::new(),
        format!("Network access: {network}"),
        format!("Environment overrides: {env_count}"),
        format!("Matching prefix: {prefix}"),
        String::new(),
        option_line(0, "1. Allow once - run only this command now".to_string()),
        option_line(
            1,
            format!(
                "2. Allow matching prefix - trust commands that start with `{prefix}` for this session"
            ),
        ),
        option_line(
            2,
            "3. Allow for this session - stop asking for shell commands until the session changes"
                .to_string(),
        ),
        option_line(3, "4. Deny - do not run the command".to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{
        pending_interaction_hint_text, pending_interaction_shortcut_text,
        shell_approval_text_lines, status_command_approval_text, status_plan_approval_text,
    };
    use crate::config::ConfigManager;
    use crate::tools::bash::BashCommandInput;
    use crate::tui::state::{
        InteractionKind, PendingApprovalSnapshot, PendingInteractionSnapshot, TuiApp,
    };

    #[test]
    fn plan_approval_text_uses_action_oriented_choice_labels() {
        let temp = tempdir().unwrap();
        let app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");

        let rendered = status_plan_approval_text(&app);
        assert!(rendered.contains("Plan ready for implementation."));
        assert!(rendered.contains("1. Start implementation now"));
        assert!(rendered.contains("2. Continue planning and refine the plan"));
        assert!(!rendered.contains("1. yes"));
    }

    #[test]
    fn pending_interaction_hint_text_describes_approval_scope() {
        assert_eq!(
            pending_interaction_hint_text(
                crate::tui::state::ActivePendingInteractionKind::PlanApproval
            ),
            "plan ready  1 start implementation  2 continue planning"
        );
        assert_eq!(
            pending_interaction_hint_text(
                crate::tui::state::ActivePendingInteractionKind::ShellApproval
            ),
            "approval required  up/down select  enter apply  1-4 shortcut"
        );
    }

    #[test]
    fn pending_interaction_shortcut_text_avoids_duplicate_shell_approval_footer() {
        assert_eq!(
            pending_interaction_shortcut_text(
                crate::tui::state::ActivePendingInteractionKind::PlanApproval
            ),
            Some("1 start implementation  2 continue planning")
        );
        assert_eq!(
            pending_interaction_shortcut_text(
                crate::tui::state::ActivePendingInteractionKind::ShellApproval
            ),
            None
        );
    }

    #[test]
    fn command_approval_text_uses_explicit_decision_labels() {
        let temp = tempdir().unwrap();
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.snapshot
            .pending_interactions
            .push(PendingInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: "Pending Approval".into(),
                summary: "cargo check".into(),
                options: Vec::new(),
                note: None,
                approval: Some(PendingApprovalSnapshot {
                    tool_use_id: "toolu_123".into(),
                    command: "cargo check".into(),
                    allow_net: false,
                    payload: BashCommandInput {
                        command: Some("cargo check".into()),
                        cwd: Some("/repo".into()),
                        ..Default::default()
                    },
                }),
                source: None,
            });

        let rendered = status_command_approval_text(&app);
        assert!(rendered.contains("Review this shell command before RARA runs it."));
        assert!(rendered.contains("1. Allow once"));
        assert!(rendered.contains("2. Allow matching prefix"));
        assert!(rendered.contains("3. Allow for this session"));
        assert!(rendered.contains("4. Deny"));
        assert!(!rendered.contains("1. yes"));
        assert!(!rendered.contains("3. on"));
        assert!(!rendered.contains("4. no"));
    }

    #[test]
    fn shell_approval_text_lines_mark_selected_option_only_in_card_mode() {
        let temp = tempdir().unwrap();
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.snapshot
            .pending_interactions
            .push(PendingInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: "Pending Approval".into(),
                summary: "cargo check".into(),
                options: Vec::new(),
                note: None,
                approval: Some(PendingApprovalSnapshot {
                    tool_use_id: "toolu_123".into(),
                    command: "cargo check".into(),
                    allow_net: false,
                    payload: BashCommandInput {
                        command: Some("cargo check".into()),
                        cwd: Some("/repo".into()),
                        ..Default::default()
                    },
                }),
                source: None,
            });

        let selected = shell_approval_text_lines(&app, Some(2)).join("\n");
        assert!(selected.contains("> 3. Allow for this session"));
        assert!(!selected.contains("> 1. Allow once"));

        let plain = shell_approval_text_lines(&app, None).join("\n");
        assert!(!plain.contains("> 3. Allow for this session"));
    }
}
