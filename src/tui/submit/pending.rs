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
            input_control::answer_plan_approval(app, agent_slot, index == 0);
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
    match input.trim().parse::<usize>() {
        Ok(index @ 1..=9) => Some(index - 1),
        _ => None,
    }
}
