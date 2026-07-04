use crate::agent::Agent;
use crate::runtime_control::ShellApprovalDecision;
use crate::tui::input_control;
use crate::tui::state::{ActivePendingInteractionKind, TuiApp};

pub(super) fn handle_pending_option_submit(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    trimmed: &str,
) -> bool {
    let Some(index) = pending_option_index_from_text(trimmed) else {
        return false;
    };
    if index >= app.active_pending_option_count() {
        return false;
    }
    let Some(interaction) = app.active_pending_interaction() else {
        return false;
    };
    match interaction.kind {
        ActivePendingInteractionKind::PlanApproval => {
            if let Some(decision) = input_control::plan_approval_decision_for_index(index) {
                let feedback = pending_option_feedback_from_text(trimmed);
                input_control::answer_plan_approval_with_feedback(
                    app, agent_slot, decision, feedback,
                );
            }
            true
        }
        ActivePendingInteractionKind::ShellApproval => {
            let selection = match index {
                0 => ShellApprovalDecision::Once,
                1 => ShellApprovalDecision::Prefix,
                2 => ShellApprovalDecision::Always,
                _ => ShellApprovalDecision::Suggestion,
            };
            input_control::answer_shell_approval(app, agent_slot, selection);
            true
        }
        ActivePendingInteractionKind::PlanningQuestion
        | ActivePendingInteractionKind::ExplorationQuestion
        | ActivePendingInteractionKind::SubAgentQuestion
        | ActivePendingInteractionKind::RequestInput => {
            if let Some(label) = app.pending_question_option_label(index) {
                if let Some(agent) = agent_slot.take() {
                    input_control::answer_pending_input(app, agent_slot, agent, label);
                } else {
                    app.push_notice("Request input is still preparing. Try the shortcut again.");
                }
                return true;
            }
            false
        }
    }
}

fn pending_option_index_from_text(input: &str) -> Option<usize> {
    let token = input.split_whitespace().next().unwrap_or_default();
    match token.parse::<usize>() {
        Ok(index @ 1..=9) => Some(index - 1),
        _ => None,
    }
}

fn pending_option_feedback_from_text(input: &str) -> Option<String> {
    input
        .trim()
        .split_once(char::is_whitespace)
        .map(|(_, feedback)| feedback.trim().to_string())
        .filter(|feedback| !feedback.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{pending_option_feedback_from_text, pending_option_index_from_text};

    #[test]
    fn parses_option_index_from_first_token() {
        assert_eq!(pending_option_index_from_text("2 add validation"), Some(1));
        assert_eq!(pending_option_index_from_text(" 3 reject"), Some(2));
        assert_eq!(pending_option_index_from_text("10 too high"), None);
        assert_eq!(pending_option_index_from_text("keep planning"), None);
    }

    #[test]
    fn extracts_feedback_after_option_token() {
        assert_eq!(
            pending_option_feedback_from_text("2 add validation").as_deref(),
            Some("add validation")
        );
        assert_eq!(
            pending_option_feedback_from_text(" 3   explain the risk ").as_deref(),
            Some("explain the risk")
        );
        assert_eq!(pending_option_feedback_from_text("1"), None);
        assert_eq!(pending_option_feedback_from_text("2   "), None);
    }
}
