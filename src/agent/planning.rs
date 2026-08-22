use anyhow::anyhow;
use rara_tools::planning::{ENTER_PLAN_MODE_TOOL_NAME, EXIT_PLAN_MODE_TOOL_NAME};
use rara_tools::tool::ToolProgressEvent;
use sha2::{Digest, Sha256};

use super::*;
use crate::tasklist::{TaskListStore, canonical_task_list_id};
use crate::tools::bash::BashCommandInput;
use crate::tools::tasklist::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
use crate::tools::todo::TODO_WRITE_TOOL_NAME;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub step: String,
    pub status: PlanStepStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUserInput {
    pub question: String,
    pub options: Vec<(String, String)>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub tool_use_id: String,
    pub request: BashCommandInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedInteraction {
    pub title: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default)]
pub(super) struct InspectionProgress {
    pub(super) list_calls: usize,
    pub(super) source_reads: usize,
    pub(super) config_reads: usize,
    pub(super) instruction_reads: usize,
    pub(super) delegated_inspections: usize,
}

#[derive(Clone, Copy)]
pub(super) enum RuntimeContinuationPhase {
    ToolResultsAvailable,
    PlanContinuationRequired,
    PlanExitRepairRequired,
    ExecutionContinuationRequired,
    ReasoningOnlyContinuationRequired,
    PlanApproved,
}

#[derive(Serialize)]
struct RuntimeContinuation<'a> {
    phase: &'a str,
    mode: &'a str,
    agentic_turns: usize,
    inspection: RuntimeInspectionSnapshot,
    plan: RuntimePlanSnapshot,
    pending_interaction: &'a str,
    instructions: Vec<&'a str>,
}

#[derive(Serialize)]
struct RuntimeInspectionSnapshot {
    list_calls: usize,
    source_reads: usize,
    config_reads: usize,
    instruction_reads: usize,
    delegated_inspections: usize,
    has_minimum_review_evidence: bool,
}

#[derive(Serialize)]
struct RuntimePlanSnapshot {
    total_steps: usize,
    pending_steps: usize,
    in_progress_steps: usize,
    completed_steps: usize,
}

impl InspectionProgress {
    pub(super) fn record_tool(&mut self, name: &str, input: &Value) {
        match name {
            "list_files" => {
                self.list_calls += 1;
            }
            "explore_agent" | "plan_agent" => {
                self.delegated_inspections += 1;
            }
            "read_file" => {
                let path = input
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .replace('\\', "/");
                if path.starts_with("src/") || path.ends_with(".rs") {
                    self.source_reads += 1;
                } else if path.ends_with("cargo.toml")
                    || path.ends_with("cargo.lock")
                    || path.ends_with(".toml")
                {
                    self.config_reads += 1;
                } else if path.ends_with("agents.md")
                    || path.ends_with("readme.md")
                    || path.ends_with(".rara/memory.md")
                    || path.ends_with(".md")
                {
                    self.instruction_reads += 1;
                }
            }
            _ => {}
        }
    }

    pub(super) fn has_minimum_review_evidence(&self) -> bool {
        self.source_reads >= 2
            || (self.source_reads >= 1
                && self.list_calls >= 1
                && (self.config_reads >= 1 || self.instruction_reads >= 1))
    }

    pub(super) fn has_any_evidence(&self) -> bool {
        self.list_calls > 0
            || self.source_reads > 0
            || self.config_reads > 0
            || self.instruction_reads > 0
            || self.delegated_inspections > 0
    }
}

impl Agent {
    pub fn last_query_produced_plan(&self) -> bool {
        self.last_query_plan_updated
    }

    pub fn has_pending_plan_exit_approval(&self) -> bool {
        self.pending_plan_exit_tool_id.is_some()
    }

    pub fn pending_plan_exit_tool_id(&self) -> Option<&str> {
        self.pending_plan_exit_tool_id.as_deref()
    }

    pub fn restore_pending_plan_exit_approval(&mut self, tool_id: &str) {
        if tool_id.trim().is_empty() {
            return;
        }
        self.pending_plan_exit_tool_id = Some(tool_id.to_string());
        self.execution_mode = AgentExecutionMode::Plan;
    }

    pub fn set_execution_mode(&mut self, mode: AgentExecutionMode) {
        self.execution_mode = mode;
    }

    pub fn set_max_turns(&mut self, max_turns: usize) {
        self.max_turns = if max_turns == 0 {
            None
        } else {
            Some(max_turns)
        };
    }

    pub fn set_token_budget(&mut self, token_budget: Option<u32>) {
        self.token_budget = token_budget;
        self.token_budget_exhausted = false;
    }

    pub fn total_model_tokens(&self) -> u32 {
        self.total_input_tokens
            .saturating_add(self.total_output_tokens)
    }

    pub fn execution_mode_label(&self) -> &'static str {
        match self.execution_mode {
            AgentExecutionMode::Execute => "execute",
            AgentExecutionMode::Plan => "plan",
            AgentExecutionMode::Review => "review",
        }
    }

    pub fn set_bash_approval_mode(&mut self, mode: BashApprovalMode) {
        self.bash_approval_mode = mode;
    }

    pub fn set_full_access_mode(&mut self, full_access: bool) {
        self.full_access_mode = full_access;
    }

    pub fn set_task_list_id(&mut self, task_list_id: impl AsRef<str>) -> String {
        let task_list_id = canonical_task_list_id(task_list_id.as_ref());
        self.task_list_id = task_list_id.clone();
        self.refresh_shared_task_tool_defaults();
        task_list_id
    }

    fn refresh_shared_task_tool_defaults(&mut self) {
        let store = std::sync::Arc::new(TaskListStore::new(self.workspace.rara_dir.join("tasks")));
        let task_list_id = self.task_list_id.clone();
        if self.tool_manager.get_tool("task_create").is_some() {
            self.tool_manager.register(Box::new(TaskCreateTool {
                store: store.clone(),
                default_task_list_id: task_list_id.clone(),
            }));
        }
        if self.tool_manager.get_tool("task_list").is_some() {
            self.tool_manager.register(Box::new(TaskListTool {
                store: store.clone(),
                default_task_list_id: task_list_id.clone(),
            }));
        }
        if self.tool_manager.get_tool("task_update").is_some() {
            self.tool_manager.register(Box::new(TaskUpdateTool {
                store: store.clone(),
                default_task_list_id: task_list_id.clone(),
            }));
        }
        if self.tool_manager.get_tool("task_get").is_some() {
            self.tool_manager.register(Box::new(TaskGetTool {
                store,
                default_task_list_id: task_list_id,
            }));
        }
    }

    pub fn is_bash_prefix_approved(&self, request: &BashCommandInput) -> bool {
        request.is_allowed_by_approval_prefixes(&self.approved_bash_prefixes)
    }

    pub fn clear_completed_interactions(&mut self) {
        self.completed_user_input = None;
        self.completed_approval = None;
    }

    pub fn consume_pending_user_input(&mut self, answer: &str) {
        if let Some(pending) = self.pending_user_input.take() {
            self.completed_user_input = Some(CompletedInteraction {
                title: pending.question,
                summary: format!("Answered with: {}", answer.trim()),
            });
        }
    }

    pub async fn answer_pending_approval_with_events<F>(
        &mut self,
        selection: BashApprovalDecision,
        output_mode: AgentOutputMode,
        mut report: F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let pending = self
            .pending_approval
            .clone()
            .ok_or_else(|| anyhow!("No pending approval to answer"))?;

        self.pending_approval = None;
        self.pending_user_input = None;
        self.completed_approval = None;
        report(AgentEvent::ApprovalAnswered {
            approval_id: pending.tool_use_id.clone(),
            approved: !matches!(selection, BashApprovalDecision::Suggestion),
        });

        match selection {
            BashApprovalDecision::Once => {
                self.completed_approval = Some(CompletedInteraction {
                    title: "Bash approval".to_string(),
                    summary: format!("Approved once for command: {}", pending.request.summary()),
                });
                self.execute_pending_bash(pending, false, output_mode, &mut report)
                    .await?;
            }
            BashApprovalDecision::Prefix => {
                let prefix = pending
                    .request
                    .approval_prefix()
                    .unwrap_or_else(|| pending.request.summary());
                if !self.approved_bash_prefixes.contains(&prefix) {
                    self.approved_bash_prefixes.push(prefix.clone());
                }
                self.completed_approval = Some(CompletedInteraction {
                    title: "Bash approval".to_string(),
                    summary: format!("Approved prefix for session: {}", prefix),
                });
                self.execute_pending_bash(pending, false, output_mode, &mut report)
                    .await?;
            }
            BashApprovalDecision::Always => {
                self.bash_approval_mode = BashApprovalMode::Always;
                self.completed_approval = Some(CompletedInteraction {
                    title: "Bash approval".to_string(),
                    summary: format!("Approved for session: {}", pending.request.summary()),
                });
                self.execute_pending_bash(pending, true, output_mode, &mut report)
                    .await?;
            }
            BashApprovalDecision::Suggestion => {
                self.completed_approval = Some(CompletedInteraction {
                    title: "Bash approval".to_string(),
                    summary: format!("Rejected command: {}", pending.request.summary()),
                });
                let error_text = format!(
                    "Error: bash command rejected by user. The command was not run: {}",
                    pending.request.summary()
                );
                report(AgentEvent::ToolResult {
                    call_id: pending.tool_use_id.clone(),
                    name: "bash".to_string(),
                    content: error_text.clone(),
                    is_error: true,
                });
                self.push_history_message(tool_result_message(
                    &pending.tool_use_id,
                    error_text,
                    true,
                ));
                self.push_history_message(self.runtime_continuation_message(
                    RuntimeContinuationPhase::ToolResultsAvailable,
                    0,
                ));
                self.checkpoint_session()?;
                self.run_agent_loop(output_mode, &mut report).await?;
            }
        }

        self.checkpoint_session()?;
        Ok(())
    }

    async fn execute_pending_bash<F>(
        &mut self,
        pending: PendingApproval,
        keep_always: bool,
        output_mode: AgentOutputMode,
        report: &mut F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let input = pending.request.to_value();
        report(AgentEvent::ToolUse {
            call_id: pending.tool_use_id.clone(),
            name: "bash".to_string(),
            input: input.clone(),
        });
        let tool = self
            .tool_manager
            .get_tool("bash")
            .ok_or_else(|| anyhow!("bash tool is unavailable"))?;
        let status_detail = BashCommandInput::from_value(input.clone())
            .map(|request| format!("Running approved shell command: {}", request.summary()))
            .unwrap_or_else(|_| "Running approved bash command.".to_string());
        report(AgentEvent::Status(status_detail));
        match tool
            .call_with_context_events(
                input.clone(),
                self.tool_call_context(&pending.tool_use_id),
                &mut |progress| match progress {
                    ToolProgressEvent::Output { stream, chunk } => {
                        report(AgentEvent::ToolProgress {
                            call_id: pending.tool_use_id.clone(),
                            name: "bash".to_string(),
                            stream,
                            chunk,
                        });
                    }
                },
            )
            .await
        {
            Ok(result) => {
                let result_text = self.tool_result_store.compact_result(
                    "bash",
                    &pending.tool_use_id,
                    &input,
                    &result,
                )?;
                report(AgentEvent::ToolResult {
                    call_id: pending.tool_use_id.clone(),
                    name: "bash".to_string(),
                    content: result_text.clone(),
                    is_error: false,
                });
                self.push_history_message(tool_result_message(
                    &pending.tool_use_id,
                    result_text,
                    false,
                ));
                self.checkpoint_session()?;
            }
            Err(err) => {
                let error_text = format!("Error: {}", err);
                report(AgentEvent::ToolResult {
                    call_id: pending.tool_use_id.clone(),
                    name: "bash".to_string(),
                    content: error_text.clone(),
                    is_error: true,
                });
                self.push_history_message(tool_result_message(
                    &pending.tool_use_id,
                    error_text,
                    true,
                ));
                self.checkpoint_session()?;
            }
        }
        self.push_history_message(
            self.runtime_continuation_message(RuntimeContinuationPhase::ToolResultsAvailable, 1),
        );
        self.checkpoint_session()?;
        if !keep_always {
            self.bash_approval_mode = BashApprovalMode::Suggestion;
        }
        self.run_agent_loop(output_mode, report).await
    }

    pub(super) fn extend_history_for_next_turn<F>(
        &mut self,
        tool_results: Vec<Message>,
        report: &mut F,
        agentic_turns: usize,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.extend_history_messages(tool_results);
        report(AgentEvent::Status(
            "Tool results recorded. Advancing to the next agent step.".to_string(),
        ));
        self.push_history_message(self.runtime_continuation_message(
            RuntimeContinuationPhase::ToolResultsAvailable,
            agentic_turns,
        ));
        self.checkpoint_session()
    }

    pub(super) fn visible_tool_schemas(&self) -> Vec<Value> {
        self.tool_manager
            .get_schemas_filtered(|name| self.is_tool_allowed_in_current_mode(name))
    }

    pub(super) fn is_tool_allowed_in_current_mode(&self, name: &str) -> bool {
        match self.execution_mode {
            AgentExecutionMode::Execute => name != EXIT_PLAN_MODE_TOOL_NAME,
            AgentExecutionMode::Review => !matches!(
                name,
                ENTER_PLAN_MODE_TOOL_NAME
                    | EXIT_PLAN_MODE_TOOL_NAME
                    | "write_file"
                    | "replace"
                    | "replace_lines"
                    | "apply_patch"
                    | "update_project_memory"
                    | TODO_WRITE_TOOL_NAME
                    | "spawn_agent"
                    | "team_create"
            ),
            AgentExecutionMode::Plan => !matches!(
                name,
                ENTER_PLAN_MODE_TOOL_NAME
                    | "write_file"
                    | "replace"
                    | "replace_lines"
                    | "apply_patch"
                    | "update_project_memory"
                    | TODO_WRITE_TOOL_NAME
                    | "spawn_agent"
                    | "team_create"
            ),
        }
    }

    pub async fn resume_after_plan_approval_with_events<F>(
        &mut self,
        continue_planning: bool,
        output_mode: AgentOutputMode,
        report: F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.resume_after_plan_approval_with_feedback_events(
            continue_planning,
            None,
            output_mode,
            report,
        )
        .await
    }

    pub async fn resume_after_plan_approval_with_feedback_events<F>(
        &mut self,
        continue_planning: bool,
        feedback: Option<&str>,
        output_mode: AgentOutputMode,
        mut report: F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.pending_user_input = None;
        self.pending_approval = None;
        let pending_plan_exit_tool_id = self.pending_plan_exit_tool_id.take();

        if let Some(approval_id) = pending_plan_exit_tool_id.as_ref() {
            report(AgentEvent::ApprovalAnswered {
                approval_id: approval_id.clone(),
                approved: !continue_planning,
            });
        }

        if continue_planning {
            self.execution_mode = AgentExecutionMode::Plan;
            report(AgentEvent::Status(
                "Continuing plan refinement from the current plan state.".to_string(),
            ));
            if let Some(tool_id) = pending_plan_exit_tool_id {
                let feedback = feedback
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| format!("\nUser feedback: {value}"))
                    .unwrap_or_default();
                self.push_history_message(tool_result_message(
                    &tool_id,
                    format!(
                        "User chose to continue planning. Revise the plan and call exit_plan_mode again when it is ready for approval.{feedback}"
                    ),
                    false,
                ));
            }
            self.push_history_message(self.runtime_continuation_message(
                RuntimeContinuationPhase::PlanContinuationRequired,
                0,
            ));
            self.checkpoint_session()?;
        } else {
            self.execution_mode = AgentExecutionMode::Execute;
            report(AgentEvent::Status(
                "Plan approved. Continuing with implementation.".to_string(),
            ));
            if let Some(tool_id) = pending_plan_exit_tool_id {
                let plan_file_path = self.session_manager.plan_file_path(&self.session_id);
                let plan_reference = format!(
                    "User has approved your plan. You can now start coding.\n\nYour plan has been saved to: {}\nYou can refer back to it if needed during implementation.\n\n## Approved Plan:\n{}",
                    plan_file_path.display(),
                    self.current_plan_markdown()
                );
                self.push_history_message(tool_result_message(&tool_id, plan_reference, false));
            }
            self.push_history_message(
                self.runtime_continuation_message(RuntimeContinuationPhase::PlanApproved, 0),
            );
            self.checkpoint_session()?;
        }

        report(AgentEvent::PlanUpdated {
            steps: self.current_plan.clone(),
            explanation: self.plan_explanation.clone(),
        });

        self.run_agent_loop(output_mode, &mut report).await
    }

    pub fn reject_pending_plan_approval(&mut self, feedback: Option<&str>) -> Result<()> {
        self.pending_user_input = None;
        self.pending_approval = None;
        let pending_plan_exit_tool_id = self.pending_plan_exit_tool_id.take();
        self.execution_mode = AgentExecutionMode::Execute;
        if let Some(tool_id) = pending_plan_exit_tool_id {
            let feedback = feedback
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("\nUser feedback: {value}"))
                .unwrap_or_default();
            self.push_history_message(tool_result_message(
                &tool_id,
                format!("User rejected the plan. Do not start implementation.{feedback}"),
                true,
            ));
        }
        self.checkpoint_session()
    }

    pub(super) fn capture_plan_from_text(&mut self, text: &str) -> Result<bool> {
        let Some((steps, explanation)) = parse_plan_block(text) else {
            self.pending_user_input = parse_request_user_input_block(text);
            return Ok(false);
        };
        if !steps.is_empty() {
            self.current_plan = steps;
        }
        self.plan_explanation = explanation;
        self.pending_user_input = parse_request_user_input_block(text);
        Ok(true)
    }

    pub(super) fn current_plan_markdown(&self) -> String {
        let mut lines = Vec::new();
        if let Some(explanation) = self.plan_explanation.as_ref() {
            let trimmed = explanation.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
                lines.push(String::new());
            }
        }
        for step in &self.current_plan {
            let status = match step.status {
                PlanStepStatus::Pending => "pending",
                PlanStepStatus::InProgress => "in_progress",
                PlanStepStatus::Completed => "completed",
            };
            lines.push(format!("- [{status}] {}", step.step));
        }
        lines.join("\n")
    }

    pub(crate) fn current_plan_hash(&self) -> String {
        use std::fmt::Write as _;

        let digest = Sha256::digest(self.current_plan_markdown().as_bytes());
        let mut hex = String::with_capacity(64);
        for byte in digest.as_slice() {
            write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
        }
        format!("sha256:{hex}")
    }

    pub(super) fn save_current_plan_file(&self) -> Result<()> {
        self.session_manager
            .save_plan_file(&self.session_id, &self.current_plan_markdown())
    }

    pub(super) fn should_continue_plan_without_tools(
        &self,
        plan_updated: bool,
        continue_inspection: bool,
        had_text_response: bool,
        had_reasoning_response: bool,
        agentic_turns: usize,
    ) -> bool {
        let shallow_initial_plan =
            plan_updated && agentic_turns == 0 && self.current_plan.len() <= 1;
        let reasoning_only_turn =
            Self::is_reasoning_only_turn(had_text_response, had_reasoning_response);
        let has_inspection_evidence = self.inspection_progress.has_any_evidence();
        let still_missing_inspection_evidence = agentic_turns > 0
            && !plan_updated
            && has_inspection_evidence
            && !self.inspection_progress.has_minimum_review_evidence();
        matches!(self.execution_mode, AgentExecutionMode::Plan)
            && (continue_inspection
                || shallow_initial_plan
                || still_missing_inspection_evidence
                || reasoning_only_turn)
            && self.pending_user_input.is_none()
            && self.pending_approval.is_none()
            && (continue_inspection
                || has_inspection_evidence
                || !self.current_plan.is_empty()
                || had_text_response
                || reasoning_only_turn)
    }

    pub(super) fn should_continue_execute_without_tools(
        &self,
        continue_inspection: bool,
        had_text_response: bool,
        had_reasoning_response: bool,
    ) -> bool {
        let reasoning_only_turn =
            Self::is_reasoning_only_turn(had_text_response, had_reasoning_response);
        matches!(self.execution_mode, AgentExecutionMode::Execute)
            && (continue_inspection || reasoning_only_turn)
            && self.pending_user_input.is_none()
            && self.pending_approval.is_none()
    }

    pub(super) fn is_reasoning_only_turn(
        had_text_response: bool,
        had_reasoning_response: bool,
    ) -> bool {
        had_reasoning_response && !had_text_response
    }

    pub(super) fn ensure_active_plan_step(&mut self) {
        if !matches!(self.execution_mode, AgentExecutionMode::Execute)
            || self.current_plan.is_empty()
        {
            return;
        }
        if self
            .current_plan
            .iter()
            .any(|step| matches!(step.status, PlanStepStatus::InProgress))
        {
            return;
        }
        if let Some(step) = self
            .current_plan
            .iter_mut()
            .find(|step| matches!(step.status, PlanStepStatus::Pending))
        {
            step.status = PlanStepStatus::InProgress;
        }
    }

    pub(super) fn advance_plan_step(&mut self) {
        if !matches!(self.execution_mode, AgentExecutionMode::Execute)
            || self.current_plan.is_empty()
        {
            return;
        }
        if let Some(step) = self
            .current_plan
            .iter_mut()
            .find(|step| matches!(step.status, PlanStepStatus::InProgress))
        {
            step.status = PlanStepStatus::Completed;
        }
        if let Some(step) = self
            .current_plan
            .iter_mut()
            .find(|step| matches!(step.status, PlanStepStatus::Pending))
        {
            step.status = PlanStepStatus::InProgress;
        }
    }

    pub(super) fn complete_active_plan_step(&mut self) {
        if !matches!(self.execution_mode, AgentExecutionMode::Execute)
            || self.current_plan.is_empty()
        {
            return;
        }
        if let Some(step) = self
            .current_plan
            .iter_mut()
            .find(|step| matches!(step.status, PlanStepStatus::InProgress))
        {
            step.status = PlanStepStatus::Completed;
        }
    }

    pub(super) fn runtime_continuation_message(
        &self,
        phase: RuntimeContinuationPhase,
        agentic_turns: usize,
    ) -> Message {
        let pending_interaction = if self.pending_approval.is_some() {
            "approval"
        } else if self.pending_user_input.is_some() {
            "request_user_input"
        } else {
            "none"
        };
        let payload = RuntimeContinuation {
            phase: phase.label(),
            mode: self.execution_mode_label(),
            agentic_turns,
            inspection: RuntimeInspectionSnapshot {
                list_calls: self.inspection_progress.list_calls,
                source_reads: self.inspection_progress.source_reads,
                config_reads: self.inspection_progress.config_reads,
                instruction_reads: self.inspection_progress.instruction_reads,
                delegated_inspections: self.inspection_progress.delegated_inspections,
                has_minimum_review_evidence: self.inspection_progress.has_minimum_review_evidence(),
            },
            plan: RuntimePlanSnapshot {
                total_steps: self.current_plan.len(),
                pending_steps: self
                    .current_plan
                    .iter()
                    .filter(|step| matches!(step.status, PlanStepStatus::Pending))
                    .count(),
                in_progress_steps: self
                    .current_plan
                    .iter()
                    .filter(|step| matches!(step.status, PlanStepStatus::InProgress))
                    .count(),
                completed_steps: self
                    .current_plan
                    .iter()
                    .filter(|step| matches!(step.status, PlanStepStatus::Completed))
                    .count(),
            },
            pending_interaction,
            instructions: phase.instructions(),
        };
        let payload = serde_json::to_string_pretty(&payload)
            .unwrap_or_else(|_| "{\"phase\":\"tool_results_available\"}".to_string());
        Message {
            role: "user".to_string(),
            content: json!([{"type": "text", "text": format!("<agent_runtime>\n{payload}\n</agent_runtime>")}]),
        }
    }
}

mod parser;

pub(super) use parser::{
    has_unclosed_proposed_plan_block, parse_exit_plan_tool_input, parse_plan_block,
    parse_request_user_input_block,
};
impl RuntimeContinuationPhase {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ToolResultsAvailable => "tool_results_available",
            Self::PlanContinuationRequired => "plan_continuation_required",
            Self::PlanExitRepairRequired => "plan_exit_repair_required",
            Self::ExecutionContinuationRequired => "execution_continuation_required",
            Self::ReasoningOnlyContinuationRequired => "reasoning_only_continuation_required",
            Self::PlanApproved => "plan_approved",
        }
    }

    fn instructions(self) -> Vec<&'static str> {
        match self {
            Self::ToolResultsAvailable => vec![
                "Continue the same task immediately.",
                "Review the tool results already present in the conversation.",
                "Either call the next tool directly, or provide the final answer.",
                "Do not ask the user to continue.",
                "Do not repeat tool results verbatim.",
            ],
            Self::PlanContinuationRequired => vec![
                "Continue planning immediately.",
                "Use read-only tools to inspect the repository before stopping.",
                "If the user asked for analysis or recommendations only, provide the final answer without a <proposed_plan> block.",
                "Use <proposed_plan> only when you are requesting approval to implement a concrete plan.",
                "Use <request_user_input> when a key decision blocks the answer.",
                "Use <continue_inspection/> when more repository inspection is still required.",
                "Do not ask the user to continue.",
            ],
            Self::PlanExitRepairRequired => vec![
                "The previous exit_plan_mode call failed because the same assistant response did not contain a complete <proposed_plan>...</proposed_plan> block.",
                "Continue the same planning task immediately.",
                "If implementation approval is still needed, start the next response with <proposed_plan>, write summary:, steps:, and validation: fields, close it with the exact </proposed_plan> tag, and then call exit_plan_mode.",
                "Do not use Markdown headings, plain bullets, or ordinary prose as a substitute for the <proposed_plan> block.",
                "If no implementation approval is needed, provide the final answer without calling exit_plan_mode.",
                "Do not ask the user to continue.",
            ],
            Self::ExecutionContinuationRequired => vec![
                "Continue the same task immediately.",
                "Keep gathering the next relevant code context now.",
                "If you mentioned a next file or next inspection step, call the tool for it directly.",
                "Only stop when you can provide concrete, evidence-based suggestions or when structured user input is required.",
                "Do not ask the user to continue.",
            ],
            Self::ReasoningOnlyContinuationRequired => vec![
                "The previous model turn produced internal reasoning but no visible assistant text and no tool call.",
                "The task is NOT complete. You MUST produce visible text or call a tool in this turn.",
                "Do not produce another reasoning-only response — the session will stop if you do.",
                "Either call the next needed tool directly, or provide a visible final answer.",
                "Do not ask the user to continue.",
            ],
            Self::PlanApproved => vec![
                "The current plan was approved. Continue the same task immediately.",
                "Implement the approved plan using the existing plan state and repository context.",
                "Do not restate the plan back to the user unless a short reminder is necessary.",
                "Start with the next concrete implementation step or tool call.",
                "After the implementation is complete, review your own changes for correctness, scope control, and missing validation before giving the final answer.",
                "Do not ask the user to continue.",
            ],
        }
    }
}

pub(super) fn strip_continue_inspection_control(text: &str) -> (String, bool) {
    const CONTINUE_INSPECTION_TAG: &str = "<continue_inspection/>";
    let requested = text.contains(CONTINUE_INSPECTION_TAG);
    if !requested {
        return (text.to_string(), false);
    }

    let cleaned = text.replace(CONTINUE_INSPECTION_TAG, "");
    (cleaned, true)
}

pub(super) fn tool_result_message(tool_use_id: &str, content: String, is_error: bool) -> Message {
    let mut block = json!({
        "type": "tool_result",
        "tool_use_id": tool_use_id,
        "content": content,
    });
    if is_error {
        block["is_error"] = json!(true);
    }
    Message {
        role: "user".to_string(),
        content: json!([block]),
    }
}
