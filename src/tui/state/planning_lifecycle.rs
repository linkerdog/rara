use super::types::{CompletedInteractionSnapshot, InteractionKind, PendingInteractionSnapshot};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlanningApprovalStatus {
    #[default]
    None,
    Pending,
    Approved,
    Revising,
    Rejected,
}

impl PlanningApprovalStatus {
    pub fn label(self) -> &'static str {
        match self {
            PlanningApprovalStatus::None => "none",
            PlanningApprovalStatus::Pending => "pending",
            PlanningApprovalStatus::Approved => "approved",
            PlanningApprovalStatus::Revising => "revising",
            PlanningApprovalStatus::Rejected => "rejected",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanningApprovalDecision {
    Approve,
    ContinuePlanning,
    Reject,
}

impl PlanningApprovalDecision {
    pub fn label(self) -> &'static str {
        match self {
            PlanningApprovalDecision::Approve => "approve",
            PlanningApprovalDecision::ContinuePlanning => "continue_planning",
            PlanningApprovalDecision::Reject => "reject",
        }
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct PlanningLifecycleSnapshot {
    pub plan_path: Option<String>,
    pub approval_status: PlanningApprovalStatus,
    pub pending_age: Option<String>,
    pub last_decision: Option<PlanningApprovalDecision>,
    pub approved_plan_revision: Option<String>,
    pub tool_use_id: Option<String>,
}

impl PlanningLifecycleSnapshot {
    pub fn from_interactions(
        session_id: &str,
        pending_interactions: &[PendingInteractionSnapshot],
        completed_interactions: &[CompletedInteractionSnapshot],
    ) -> Self {
        let plan_path = if session_id.is_empty() {
            None
        } else {
            Some(format!(".rara/sessions/{session_id}/plan.md"))
        };
        let pending_plan = pending_interactions
            .iter()
            .find(|item| item.kind == InteractionKind::PlanApproval);
        let completed_decision = completed_interactions
            .iter()
            .rev()
            .find(|item| item.kind == InteractionKind::PlanApproval)
            .and_then(plan_decision_from_completed_interaction);

        let approval_status = if pending_plan.is_some() {
            PlanningApprovalStatus::Pending
        } else {
            match completed_decision {
                Some(PlanningApprovalDecision::Approve) => PlanningApprovalStatus::Approved,
                Some(PlanningApprovalDecision::ContinuePlanning) => {
                    PlanningApprovalStatus::Revising
                }
                Some(PlanningApprovalDecision::Reject) => PlanningApprovalStatus::Rejected,
                None => PlanningApprovalStatus::None,
            }
        };

        Self {
            plan_path,
            approval_status,
            pending_age: None,
            last_decision: completed_decision,
            approved_plan_revision: None,
            tool_use_id: pending_plan
                .and_then(|item| plan_approval_tool_use_id(item.source.as_deref())),
        }
    }

    pub fn pending_age_label(&self) -> &str {
        self.pending_age.as_deref().unwrap_or("-")
    }

    pub fn last_decision_label(&self) -> &str {
        self.last_decision
            .map(PlanningApprovalDecision::label)
            .unwrap_or("-")
    }

    pub fn approved_plan_revision_label(&self) -> &str {
        self.approved_plan_revision.as_deref().unwrap_or("-")
    }

    pub fn tool_use_id_label(&self) -> &str {
        self.tool_use_id.as_deref().unwrap_or("-")
    }
}

fn plan_decision_from_completed_interaction(
    interaction: &CompletedInteractionSnapshot,
) -> Option<PlanningApprovalDecision> {
    match interaction.source.as_deref() {
        Some("plan_approval:approve") => Some(PlanningApprovalDecision::Approve),
        Some("plan_approval:continue_planning") => Some(PlanningApprovalDecision::ContinuePlanning),
        Some("plan_approval:reject") => Some(PlanningApprovalDecision::Reject),
        _ => plan_decision_from_completed_summary(&interaction.summary),
    }
}

fn plan_decision_from_completed_summary(summary: &str) -> Option<PlanningApprovalDecision> {
    match summary {
        "Approved. Starting implementation." => Some(PlanningApprovalDecision::Approve),
        "Sent back for more planning." => Some(PlanningApprovalDecision::ContinuePlanning),
        "Rejected. Implementation cancelled." => Some(PlanningApprovalDecision::Reject),
        _ => None,
    }
}

fn plan_approval_tool_use_id(source: Option<&str>) -> Option<String> {
    source
        .and_then(|value| value.strip_prefix("exit_plan_mode:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{
        CompletedInteractionSnapshot, InteractionKind, PendingInteractionSnapshot,
        PlanningApprovalDecision, PlanningApprovalStatus, PlanningLifecycleSnapshot,
    };

    #[test]
    fn derives_pending_plan_lifecycle_from_pending_interaction() {
        let snapshot = PlanningLifecycleSnapshot::from_interactions(
            "session-123",
            &[PendingInteractionSnapshot {
                kind: InteractionKind::PlanApproval,
                title: "Plan Approval".into(),
                summary: "Plan ready".into(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: Some("exit_plan_mode:tool-123".into()),
            }],
            &[CompletedInteractionSnapshot {
                kind: InteractionKind::PlanApproval,
                title: "Plan Decision".into(),
                summary: "Sent back for more planning.".into(),
                source: Some("plan_approval:continue_planning".into()),
            }],
        );

        assert_eq!(
            snapshot.plan_path.as_deref(),
            Some(".rara/sessions/session-123/plan.md")
        );
        assert_eq!(snapshot.approval_status, PlanningApprovalStatus::Pending);
        assert_eq!(
            snapshot.last_decision,
            Some(PlanningApprovalDecision::ContinuePlanning)
        );
        assert_eq!(snapshot.tool_use_id.as_deref(), Some("tool-123"));
        assert_eq!(snapshot.pending_age_label(), "-");
        assert_eq!(snapshot.approved_plan_revision_label(), "-");
    }

    #[test]
    fn derives_approved_plan_lifecycle_from_completed_interaction() {
        let snapshot = PlanningLifecycleSnapshot::from_interactions(
            "session-456",
            &[],
            &[CompletedInteractionSnapshot {
                kind: InteractionKind::PlanApproval,
                title: "Plan Decision".into(),
                summary: "Approved. Starting implementation.".into(),
                source: Some("plan_approval:approve".into()),
            }],
        );

        assert_eq!(snapshot.approval_status, PlanningApprovalStatus::Approved);
        assert_eq!(
            snapshot.last_decision,
            Some(PlanningApprovalDecision::Approve)
        );
        assert_eq!(snapshot.tool_use_id, None);
    }
}
