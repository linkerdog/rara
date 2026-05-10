use crate::tui::state::TranscriptEntry;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum CommittedCompletionKind {
    ShellApprovalCompleted,
    PlanningQuestionAnswered,
    ExplorationQuestionAnswered,
    SubAgentQuestionAnswered,
    QuestionAnswered,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommittedCompletion {
    pub(super) kind: CommittedCompletionKind,
    pub(super) message: String,
}

pub(super) fn narrative_entries(
    entries: &[TranscriptEntry],
    has_tool_activity: bool,
    is_renderable_system_message: impl Fn(&str) -> bool,
) -> Vec<&TranscriptEntry> {
    if has_tool_activity {
        if let Some(agent) = entries.iter().rev().find(|entry| entry.role == "Agent") {
            return vec![agent];
        }
        entries
            .iter()
            .rev()
            .find(|entry| {
                entry.role == "System" && is_renderable_system_message(entry.message.as_str())
            })
            .into_iter()
            .collect()
    } else {
        entries
            .iter()
            .filter(|entry| {
                entry.role == "Agent"
                    || (entry.role == "System"
                        && is_renderable_system_message(entry.message.as_str()))
            })
            .collect()
    }
}

pub(super) fn completion_role_kind(role: &str) -> Option<CommittedCompletionKind> {
    match role {
        "Shell Approval Completed" => Some(CommittedCompletionKind::ShellApprovalCompleted),
        "Planning Question Answered" => Some(CommittedCompletionKind::PlanningQuestionAnswered),
        "Exploration Question Answered" => {
            Some(CommittedCompletionKind::ExplorationQuestionAnswered)
        }
        "Sub-agent Question Answered" => Some(CommittedCompletionKind::SubAgentQuestionAnswered),
        "Question Answered" => Some(CommittedCompletionKind::QuestionAnswered),
        _ => None,
    }
}

pub(super) fn ordered_completion_entries(entries: &[TranscriptEntry]) -> Vec<CommittedCompletion> {
    let mut completions = entries
        .iter()
        .filter_map(|entry| {
            completion_role_kind(entry.role.as_str()).map(|kind| CommittedCompletion {
                kind,
                message: entry.message.clone(),
            })
        })
        .collect::<Vec<_>>();
    completions.sort_by_key(|completion| completion.kind);
    completions
}
