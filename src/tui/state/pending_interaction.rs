use super::{
    ActivePendingInteraction, ActivePendingInteractionKind, CompletedInteractionSnapshot,
    InteractionKind, PendingInteractionSnapshot, TuiApp,
};
use crate::agent::{AgentExecutionMode, BashApprovalMode};
use crate::tui::state::TranscriptEntry;

fn completed_interaction_role(kind: InteractionKind, source: Option<&str>) -> &'static str {
    match kind {
        InteractionKind::Approval => "Shell Approval Completed",
        InteractionKind::PlanApproval => "Plan Decision",
        InteractionKind::RequestInput => match source {
            Some("plan_agent") => "Planning Question Answered",
            Some("explore_agent") => "Exploration Question Answered",
            Some(_) => "Sub-agent Question Answered",
            None => "Question Answered",
        },
    }
}

impl TuiApp {
    pub(super) fn ensure_completed_interaction_entry(
        &mut self,
        kind: InteractionKind,
        title: &str,
        summary: &str,
        source: Option<&str>,
    ) {
        let role = completed_interaction_role(kind, source).to_string();
        let message = format!("{title}: {summary}");
        let exists = self
            .active_turn
            .entries
            .iter()
            .chain(
                self.committed_turns
                    .iter()
                    .flat_map(|turn| turn.entries.iter()),
            )
            .any(|entry| entry.role == role && entry.message == message);
        if !exists {
            self.active_turn
                .entries
                .push(TranscriptEntry::new(role, message));
        }
    }

    fn plan_approval_interaction(&self, tool_use_id: Option<&str>) -> PendingInteractionSnapshot {
        PendingInteractionSnapshot {
            kind: InteractionKind::PlanApproval,
            title: "Plan Ready".to_string(),
            summary: self
                .snapshot
                .plan_explanation
                .clone()
                .unwrap_or_else(|| "Review the proposed plan before implementation.".to_string()),
            options: Vec::new(),
            note: None,
            approval: None,
            source: tool_use_id.map(|id| format!("exit_plan_mode:{id}")),
        }
    }

    pub(super) fn set_plan_approval_interaction(
        &mut self,
        pending: bool,
        tool_use_id: Option<&str>,
    ) {
        self.snapshot
            .pending_interactions
            .retain(|item| item.kind != InteractionKind::PlanApproval);
        if pending {
            self.snapshot
                .pending_interactions
                .push(self.plan_approval_interaction(tool_use_id));
        }
    }

    pub fn set_agent_execution_mode(&mut self, mode: AgentExecutionMode) {
        self.agent_execution_mode = mode;
    }

    pub fn agent_execution_mode_label(&self) -> &'static str {
        match self.agent_execution_mode {
            AgentExecutionMode::Execute => "execute",
            AgentExecutionMode::Plan => "plan",
            AgentExecutionMode::Review => "review",
        }
    }

    pub fn permission_mode_label(&self) -> &'static str {
        self.permission_mode.label()
    }

    pub fn bash_approval_mode_label(&self) -> &'static str {
        match self.bash_approval_mode {
            BashApprovalMode::Always => "always",
            BashApprovalMode::Once => "once",
            BashApprovalMode::Suggestion => "suggestion",
        }
    }

    pub fn pending_question_option_label(&self, index: usize) -> Option<String> {
        self.pending_request_input()
            .and_then(|interaction| interaction.options.get(index))
            .map(|(label, _)| label.clone())
    }

    pub fn has_pending_approval(&self) -> bool {
        self.pending_command_approval().is_some()
    }

    pub fn has_pending_plan_approval(&self) -> bool {
        self.pending_plan_approval_interaction().is_some()
    }

    pub fn active_pending_interaction(&self) -> Option<ActivePendingInteraction<'_>> {
        if let Some(snapshot) = self.pending_plan_approval_interaction() {
            return Some(ActivePendingInteraction {
                kind: ActivePendingInteractionKind::PlanApproval,
                _snapshot: snapshot,
            });
        }
        if let Some(snapshot) = self.pending_command_approval() {
            return Some(ActivePendingInteraction {
                kind: ActivePendingInteractionKind::ShellApproval,
                _snapshot: snapshot,
            });
        }
        if let Some(snapshot) = self.pending_request_input() {
            let kind = match snapshot.source.as_deref() {
                Some("plan_agent") => ActivePendingInteractionKind::PlanningQuestion,
                Some("explore_agent") => ActivePendingInteractionKind::ExplorationQuestion,
                Some(_) => ActivePendingInteractionKind::SubAgentQuestion,
                None => ActivePendingInteractionKind::RequestInput,
            };
            return Some(ActivePendingInteraction {
                kind,
                _snapshot: snapshot,
            });
        }
        None
    }

    pub fn active_pending_option_count(&self) -> usize {
        let Some(pending) = self.active_pending_interaction() else {
            return 0;
        };
        match pending.kind {
            ActivePendingInteractionKind::PlanApproval => 3,
            ActivePendingInteractionKind::ShellApproval => 4,
            ActivePendingInteractionKind::PlanningQuestion
            | ActivePendingInteractionKind::ExplorationQuestion
            | ActivePendingInteractionKind::SubAgentQuestion
            | ActivePendingInteractionKind::RequestInput => self
                .pending_request_input()
                .map(|interaction| interaction.options.len().min(3))
                .unwrap_or(0),
        }
    }

    pub fn set_pending_plan_approval(&mut self, pending: bool) {
        self.set_pending_plan_approval_with_tool_id(pending, None);
    }

    pub fn set_pending_plan_approval_with_tool_id(
        &mut self,
        pending: bool,
        tool_use_id: Option<&str>,
    ) {
        self.set_plan_approval_interaction(pending, tool_use_id);
        self.persist_runtime_state();
    }

    pub fn pending_request_input(&self) -> Option<&PendingInteractionSnapshot> {
        self.snapshot
            .pending_interactions
            .iter()
            .find(|item| item.kind == InteractionKind::RequestInput)
    }

    pub fn has_local_pending_request_input(&self) -> bool {
        self.pending_request_input()
            .and_then(|item| item.source.as_deref())
            .is_some()
    }

    pub fn pending_command_approval(&self) -> Option<&PendingInteractionSnapshot> {
        self.snapshot
            .pending_interactions
            .iter()
            .find(|item| item.kind == InteractionKind::Approval)
    }

    pub fn clear_pending_command_approval(&mut self) {
        self.snapshot
            .pending_interactions
            .retain(|item| item.kind != InteractionKind::Approval);
        self.persist_runtime_state();
    }

    pub fn pending_plan_approval_interaction(&self) -> Option<&PendingInteractionSnapshot> {
        self.snapshot
            .pending_interactions
            .iter()
            .find(|item| item.kind == InteractionKind::PlanApproval)
    }

    pub fn completed_interaction(
        &self,
        kind: InteractionKind,
    ) -> Option<&CompletedInteractionSnapshot> {
        self.snapshot
            .completed_interactions
            .iter()
            .find(|item| item.kind == kind)
    }

    pub fn record_completed_interaction(
        &mut self,
        kind: InteractionKind,
        title: impl Into<String>,
        summary: impl Into<String>,
        source: Option<String>,
    ) {
        let title = title.into();
        let summary = summary.into();
        self.snapshot
            .completed_interactions
            .retain(|item| item.kind != kind);
        self.snapshot
            .completed_interactions
            .push(CompletedInteractionSnapshot {
                kind,
                title: title.clone(),
                summary: summary.clone(),
                source: source.clone(),
            });
        self.ensure_completed_interaction_entry(
            kind,
            title.as_str(),
            summary.as_str(),
            source.as_deref(),
        );
        self.persist_runtime_state();
    }

    pub fn record_local_request_input(
        &mut self,
        source: impl Into<String>,
        title: impl Into<String>,
        options: Vec<(String, String)>,
        note: Option<String>,
    ) {
        self.snapshot.pending_interactions.retain(|item| {
            !(item.kind == InteractionKind::RequestInput && item.source.as_deref().is_some())
        });
        let title = title.into();
        self.snapshot
            .pending_interactions
            .push(PendingInteractionSnapshot {
                kind: InteractionKind::RequestInput,
                title: title.clone(),
                summary: note.clone().unwrap_or_default(),
                options,
                note,
                approval: None,
                source: Some(source.into()),
            });
        self.bottom_pane.notice = Some(title.clone());
        self.persist_runtime_state();
    }

    pub fn clear_local_request_input(&mut self) {
        self.snapshot.pending_interactions.retain(|item| {
            !(item.kind == InteractionKind::RequestInput && item.source.as_deref().is_some())
        });
        self.persist_runtime_state();
    }
}
