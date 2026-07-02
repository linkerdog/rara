use std::sync::Arc;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolError};
use serde::Deserialize;
use serde_json::Value;

use crate::tasklist::{
    DEFAULT_TASK_LIST_ID, NewTaskRecord, TaskDetails, TaskListStore, TaskStatus, TaskUpdate,
    is_valid_task_id, task_list_entries,
};

pub struct TaskCreateTool {
    pub store: Arc<TaskListStore>,
    pub default_task_list_id: String,
}

pub struct TaskListTool {
    pub store: Arc<TaskListStore>,
    pub default_task_list_id: String,
}

pub struct TaskUpdateTool {
    pub store: Arc<TaskListStore>,
    pub default_task_list_id: String,
}

pub struct TaskGetTool {
    pub store: Arc<TaskListStore>,
    pub default_task_list_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskListInput {
    #[serde(default, alias = "taskListId")]
    task_list_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskGetInput {
    task_id: String,
    #[serde(default, alias = "taskListId")]
    task_list_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskCreateInput {
    subject: String,
    description: String,
    #[serde(default, alias = "activeForm")]
    active_form: Option<String>,
    #[serde(default)]
    metadata: serde_json::Map<String, Value>,
    #[serde(default, alias = "taskListId")]
    task_list_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskUpdateInput {
    #[serde(alias = "taskId")]
    task_id: String,
    #[serde(default, alias = "expectedRevision")]
    expected_revision: Option<u64>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, alias = "activeForm")]
    active_form: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default, alias = "claimOwner")]
    claim_owner: Option<String>,
    #[serde(default, alias = "releaseOwner")]
    release_owner: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, alias = "addBlocks")]
    add_blocks: Vec<String>,
    #[serde(default, alias = "addBlockedBy")]
    add_blocked_by: Vec<String>,
    #[serde(default)]
    metadata: serde_json::Map<String, Value>,
    #[serde(default, alias = "taskListId")]
    task_list_id: Option<String>,
}

#[tool_spec(
    name = "task_create",
    description = "Create a new shared project task with pending status. Use this for non-trivial multi-step work after checking task_list to avoid duplicates. New tasks include subject, description, optional activeForm, empty dependencies, no owner, and are readable through task_list and task_get.",
    input_schema = {
        "type": "object",
        "properties": {
            "subject": {
                "type": "string",
                "description": "Brief actionable task title in imperative form."
            },
            "description": {
                "type": "string",
                "description": "What needs to be done."
            },
            "activeForm": {
                "type": "string",
                "description": "Optional present continuous label shown while this task is in_progress, such as 'Fixing authentication bug'."
            },
            "active_form": {
                "type": "string",
                "description": "RARA-compatible alias for activeForm. Do not send together with activeForm."
            },
            "metadata": {
                "type": "object",
                "description": "Optional metadata to attach to the task.",
                "additionalProperties": true
            },
            "task_list_id": {
                "type": "string",
                "description": "Optional shared task list id. Defaults to the workspace default task list. Do not send together with taskListId."
            },
            "taskListId": {
                "type": "string",
                "description": "Claude-compatible alias for task_list_id. Do not send together with task_list_id."
            }
        },
        "required": ["subject", "description"],
        "additionalProperties": false
    }
)]
#[async_trait]
impl Tool for TaskCreateTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let input = parse_task_create_input(input)?;
        let subject = normalize_required_text(&input.subject, "subject")?;
        let description = normalize_required_text(&input.description, "description")?;
        let active_form = normalize_optional_text(input.active_form.as_deref());
        let task_list_id = input.task_list_id(&self.default_task_list_id).to_string();
        let store = self.store.clone();
        let metadata = input.metadata.into_iter().collect();
        let task = tokio::task::spawn_blocking(move || {
            store.create_task(
                &task_list_id,
                NewTaskRecord {
                    subject,
                    description,
                    active_form,
                    metadata,
                },
            )
        })
        .await
        .map_err(|err| ToolError::ExecutionFailed(format!("spawn task_create worker: {err}")))?
        .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;

        Ok(serde_json::json!({
            "task": {
                "id": task.id,
                "subject": task.subject,
            }
        }))
    }
}

#[tool_spec(
    name = "task_update",
    description = "Update a shared project task. Use task_get first to inspect the latest task state. Supports status changes including deleted, subject, description, activeForm, owner, metadata merge/delete, addBlocks, and addBlockedBy.",
    input_schema = {
        "type": "object",
        "properties": {
            "task_id": {
                "type": "string",
                "description": "Task identifier from task_list or task_get."
            },
            "taskId": {
                "type": "string",
                "description": "Claude-compatible alias for task_id. Do not send together with task_id."
            },
            "expectedRevision": {
                "type": "integer",
                "minimum": 1,
                "description": "Task revision observed from task_get or task_list. If it no longer matches, the update returns success=false without writing."
            },
            "expected_revision": {
                "type": "integer",
                "minimum": 1,
                "description": "RARA-compatible alias for expectedRevision."
            },
            "subject": {
                "type": "string",
                "description": "New imperative task title."
            },
            "description": {
                "type": "string",
                "description": "New task description."
            },
            "activeForm": {
                "type": "string",
                "description": "Present continuous label shown while this task is in_progress."
            },
            "active_form": {
                "type": "string",
                "description": "RARA-compatible alias for activeForm. Do not send together with activeForm."
            },
            "owner": {
                "type": "string",
                "description": "New task owner. Send an empty string to clear the owner."
            },
            "claimOwner": {
                "type": "string",
                "description": "Claim the task for this owner only when it is currently unowned or already owned by the same value. Do not send with owner or releaseOwner."
            },
            "claim_owner": {
                "type": "string",
                "description": "RARA-compatible alias for claimOwner."
            },
            "releaseOwner": {
                "type": "string",
                "description": "Release the task only when the current owner matches this value. Do not send with owner or claimOwner."
            },
            "release_owner": {
                "type": "string",
                "description": "RARA-compatible alias for releaseOwner."
            },
            "status": {
                "type": "string",
                "enum": ["pending", "in_progress", "completed", "deleted"],
                "description": "New task status. Use deleted to permanently remove the task."
            },
            "addBlocks": {
                "type": "array",
                "items": {"type": "string"},
                "description": "Task IDs that cannot start until this task completes."
            },
            "addBlockedBy": {
                "type": "array",
                "items": {"type": "string"},
                "description": "Task IDs that must complete before this task can start."
            },
            "metadata": {
                "type": "object",
                "description": "Metadata keys to merge into the task. Set a key to null to delete it.",
                "additionalProperties": true
            },
            "task_list_id": {
                "type": "string",
                "description": "Optional shared task list id. Defaults to the workspace default task list. Do not send together with taskListId."
            },
            "taskListId": {
                "type": "string",
                "description": "Claude-compatible alias for task_list_id. Do not send together with task_list_id."
            }
        },
        "oneOf": [
            {"required": ["task_id"]},
            {"required": ["taskId"]}
        ],
        "additionalProperties": false
    }
)]
#[async_trait]
impl Tool for TaskUpdateTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let input = parse_task_update_input(input)?;
        let task_id = input.task_id.trim().to_string();
        if !is_valid_task_id(&task_id) {
            return Err(ToolError::InvalidInput(
                "task_update requires a valid, non-empty task_id without path traversal characters"
                    .to_string(),
            ));
        }
        let task_list_id = input.task_list_id(&self.default_task_list_id).to_string();
        let update = normalize_task_update(input)?;
        let store = self.store.clone();
        let outcome =
            tokio::task::spawn_blocking(move || store.update_task(&task_list_id, &task_id, update))
                .await
                .map_err(|err| {
                    ToolError::ExecutionFailed(format!("spawn task_update worker: {err}"))
                })?
                .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;

        serde_json::to_value(outcome).map_err(|err| ToolError::ExecutionFailed(err.to_string()))
    }
}

#[tool_spec(
    name = "task_list",
    description = "List shared project tasks with their current status. Use this to find pending, unowned, unblocked work before creating duplicate tasks or after completing assigned work. Results are returned in task ID order and include only summary fields; call task_get with a specific task_id before starting or updating a task.",
    input_schema = {
        "type": "object",
        "properties": {
            "task_list_id": {
                "type": "string",
                "description": "Optional shared task list id. Defaults to the workspace default task list. Do not send together with taskListId."
            },
            "taskListId": {
                "type": "string",
                "description": "Claude-compatible alias for task_list_id. Do not send together with task_list_id."
            }
        },
        "additionalProperties": false
    }
)]
#[async_trait]
impl Tool for TaskListTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let input = parse_task_list_input(input)?;
        let tasks = self
            .store
            .list_tasks(input.task_list_id(&self.default_task_list_id))
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
        Ok(serde_json::json!({ "tasks": task_list_entries(&tasks) }))
    }
}

#[tool_spec(
    name = "task_get",
    description = "Retrieve full details for a specific shared task. Use this before starting work to verify the task is still current and that blockedBy is empty, and before any future update operation to avoid stale task state.",
    input_schema = {
        "type": "object",
        "properties": {
            "task_id": {
                "type": "string",
                "description": "Task identifier from task_list. Must be a single task id, not a path."
            },
            "task_list_id": {
                "type": "string",
                "description": "Optional shared task list id. Defaults to the workspace default task list. Do not send together with taskListId."
            },
            "taskListId": {
                "type": "string",
                "description": "Claude-compatible alias for task_list_id. Do not send together with task_list_id."
            }
        },
        "required": ["task_id"],
        "additionalProperties": false
    }
)]
#[async_trait]
impl Tool for TaskGetTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let input = parse_task_get_input(input)?;
        let task_id = input.task_id.trim();
        if !is_valid_task_id(task_id) {
            return Err(ToolError::InvalidInput(
                "task_get requires a valid, non-empty task_id without path traversal characters"
                    .to_string(),
            ));
        }

        let task = self
            .store
            .get_task(input.task_list_id(&self.default_task_list_id), task_id)
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?
            .map(TaskDetails::from);
        Ok(serde_json::json!({ "task": task }))
    }
}

impl TaskListInput {
    fn task_list_id<'a>(&'a self, default_task_list_id: &'a str) -> &'a str {
        self.task_list_id
            .as_deref()
            .map(str::trim)
            .filter(|task_list_id| !task_list_id.is_empty())
            .unwrap_or(default_task_list_id)
    }
}

impl TaskCreateInput {
    fn task_list_id<'a>(&'a self, default_task_list_id: &'a str) -> &'a str {
        self.task_list_id
            .as_deref()
            .map(str::trim)
            .filter(|task_list_id| !task_list_id.is_empty())
            .unwrap_or(default_task_list_id)
    }
}

impl TaskUpdateInput {
    fn task_list_id<'a>(&'a self, default_task_list_id: &'a str) -> &'a str {
        self.task_list_id
            .as_deref()
            .map(str::trim)
            .filter(|task_list_id| !task_list_id.is_empty())
            .unwrap_or(default_task_list_id)
    }
}

impl TaskGetInput {
    fn task_list_id<'a>(&'a self, default_task_list_id: &'a str) -> &'a str {
        self.task_list_id
            .as_deref()
            .map(str::trim)
            .filter(|task_list_id| !task_list_id.is_empty())
            .unwrap_or(default_task_list_id)
    }
}

fn parse_task_create_input(input: Value) -> Result<TaskCreateInput, ToolError> {
    reject_alias_conflict(&input, "activeForm", "active_form")?;
    reject_alias_conflict(&input, "taskListId", "task_list_id")?;
    serde_json::from_value(input).map_err(|err| ToolError::InvalidInput(err.to_string()))
}

fn parse_task_update_input(input: Value) -> Result<TaskUpdateInput, ToolError> {
    reject_alias_conflict(&input, "taskId", "task_id")?;
    reject_alias_conflict(&input, "expectedRevision", "expected_revision")?;
    reject_alias_conflict(&input, "activeForm", "active_form")?;
    reject_alias_conflict(&input, "claimOwner", "claim_owner")?;
    reject_alias_conflict(&input, "releaseOwner", "release_owner")?;
    reject_alias_conflict(&input, "taskListId", "task_list_id")?;
    serde_json::from_value(input).map_err(|err| ToolError::InvalidInput(err.to_string()))
}

fn parse_task_list_input(input: Value) -> Result<TaskListInput, ToolError> {
    let input = empty_object_for_null(input);
    serde_json::from_value(input).map_err(|err| ToolError::InvalidInput(err.to_string()))
}

fn parse_task_get_input(input: Value) -> Result<TaskGetInput, ToolError> {
    serde_json::from_value(input).map_err(|err| ToolError::InvalidInput(err.to_string()))
}

fn normalize_required_text(value: &str, field_name: &str) -> Result<String, ToolError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ToolError::InvalidInput(format!(
            "task_create requires a non-empty {field_name}"
        )));
    }
    Ok(value.to_string())
}

fn normalize_optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_task_update(input: TaskUpdateInput) -> Result<TaskUpdate, ToolError> {
    if input.owner.is_some() && (input.claim_owner.is_some() || input.release_owner.is_some()) {
        return Err(ToolError::InvalidInput(
            "task_update accepts owner or claimOwner/releaseOwner, not both".to_string(),
        ));
    }
    if input.claim_owner.is_some() && input.release_owner.is_some() {
        return Err(ToolError::InvalidInput(
            "task_update accepts claimOwner or releaseOwner, not both".to_string(),
        ));
    }
    let status = match input.status.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("pending") => Some(TaskStatus::Pending),
        Some("in_progress") => Some(TaskStatus::InProgress),
        Some("completed") => Some(TaskStatus::Completed),
        Some("deleted") => None,
        Some(other) => {
            return Err(ToolError::InvalidInput(format!(
                "task_update received invalid status '{other}'"
            )));
        }
    };
    let delete = input.status.as_deref().map(str::trim) == Some("deleted");

    Ok(TaskUpdate {
        expected_revision: input.expected_revision,
        subject: normalize_optional_text(input.subject.as_deref()),
        description: normalize_optional_text(input.description.as_deref()),
        active_form: input
            .active_form
            .as_deref()
            .map(|value| normalize_optional_text(Some(value))),
        owner: input
            .owner
            .as_deref()
            .map(|value| normalize_optional_text(Some(value))),
        claim_owner: normalize_optional_text(input.claim_owner.as_deref()),
        release_owner: normalize_optional_text(input.release_owner.as_deref()),
        status,
        metadata: input.metadata.into_iter().collect(),
        add_blocks: normalize_task_ids(input.add_blocks, "addBlocks")?,
        add_blocked_by: normalize_task_ids(input.add_blocked_by, "addBlockedBy")?,
        delete,
    })
}

fn normalize_task_ids(values: Vec<String>, field_name: &str) -> Result<Vec<String>, ToolError> {
    let mut task_ids = Vec::new();
    for value in values {
        let task_id = value.trim();
        if !is_valid_task_id(task_id) {
            return Err(ToolError::InvalidInput(format!(
                "task_update received invalid task id in {field_name}"
            )));
        }
        if !task_ids.iter().any(|existing| existing == task_id) {
            task_ids.push(task_id.to_string());
        }
    }
    Ok(task_ids)
}

fn reject_alias_conflict(input: &Value, left: &str, right: &str) -> Result<(), ToolError> {
    let Some(object) = input.as_object() else {
        return Ok(());
    };
    if object.contains_key(left) && object.contains_key(right) {
        return Err(ToolError::InvalidInput(format!(
            "task tools accept either {left} or {right}, not both"
        )));
    }
    Ok(())
}

fn empty_object_for_null(input: Value) -> Value {
    if input.is_null() {
        serde_json::json!({})
    } else {
        input
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::tasklist::DEFAULT_TASK_LIST_ID;

    #[tokio::test]
    async fn task_create_writes_pending_task() {
        let temp = tempdir().expect("tempdir");
        let tool = TaskCreateTool {
            store: Arc::new(TaskListStore::new(temp.path())),
            default_task_list_id: DEFAULT_TASK_LIST_ID.to_string(),
        };

        let output = tool
            .call(json!({
                "subject": "Implement task_create",
                "description": "Create shared task files.",
                "activeForm": "Implementing task_create",
                "metadata": {"source": "test"}
            }))
            .await
            .expect("task_create should work");

        assert_eq!(output["task"]["id"], "1");
        assert_eq!(output["task"]["subject"], "Implement task_create");

        let task = tool
            .store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect("task should load")
            .expect("task should exist");
        assert_eq!(task.status, crate::tasklist::TaskStatus::Pending);
        assert_eq!(
            task.active_form.as_deref(),
            Some("Implementing task_create")
        );
        assert_eq!(task.metadata["source"], "test");
    }

    #[tokio::test]
    async fn task_create_uses_tool_default_task_list_id() {
        let temp = tempdir().expect("tempdir");
        let tool = TaskCreateTool {
            store: Arc::new(TaskListStore::new(temp.path())),
            default_task_list_id: "team-a".to_string(),
        };

        tool.call(json!({
            "subject": "Create team task",
            "description": "Create a task in the tool default list."
        }))
        .await
        .expect("task_create should use default task list id");

        assert!(
            tool.store
                .get_task("team-a", "1")
                .expect("team task should load")
                .is_some()
        );
        assert!(
            tool.store
                .get_task(DEFAULT_TASK_LIST_ID, "1")
                .expect("default task lookup should work")
                .is_none()
        );
    }

    #[tokio::test]
    async fn task_create_rejects_empty_required_fields() {
        let temp = tempdir().expect("tempdir");
        let tool = tool_create(temp.path());

        let subject_err = tool
            .call(json!({
                "subject": " ",
                "description": "Create shared task files."
            }))
            .await
            .expect_err("empty subject should fail");
        let description_err = tool
            .call(json!({
                "subject": "Implement task_create",
                "description": " "
            }))
            .await
            .expect_err("empty description should fail");

        assert!(subject_err.to_string().contains("non-empty subject"));
        assert!(
            description_err
                .to_string()
                .contains("non-empty description")
        );
    }

    #[tokio::test]
    async fn task_create_rejects_alias_conflicts() {
        let temp = tempdir().expect("tempdir");
        let tool = tool_create(temp.path());

        let active_form_err = tool
            .call(json!({
                "subject": "Implement task_create",
                "description": "Create shared task files.",
                "activeForm": "Implementing task_create",
                "active_form": "Implementing task_create"
            }))
            .await
            .expect_err("mixed activeForm aliases should fail");
        let task_list_id_err = tool
            .call(json!({
                "subject": "Implement task_create",
                "description": "Create shared task files.",
                "taskListId": "default",
                "task_list_id": "default"
            }))
            .await
            .expect_err("mixed task list aliases should fail");

        assert!(
            active_form_err
                .to_string()
                .contains("either activeForm or active_form")
        );
        assert!(
            task_list_id_err
                .to_string()
                .contains("either taskListId or task_list_id")
        );
    }

    #[tokio::test]
    async fn task_update_changes_status_owner_and_metadata() {
        let temp = tempdir().expect("tempdir");
        let tool = tool_update(temp.path());
        tool.store
            .create_task(
                DEFAULT_TASK_LIST_ID,
                NewTaskRecord {
                    subject: "Implement task_update".to_string(),
                    description: "Update shared task files.".to_string(),
                    active_form: None,
                    metadata: Default::default(),
                },
            )
            .expect("task should be created");

        let output = tool
            .call(json!({
                "taskId": "1",
                "status": "in_progress",
                "owner": "agent-a",
                "metadata": {"priority": "high"}
            }))
            .await
            .expect("task_update should work");

        assert_eq!(output["success"], true);
        assert_eq!(output["taskId"], "1");
        assert_eq!(
            output["updatedFields"],
            json!(["metadata", "owner", "status"])
        );
        assert_eq!(output["statusChange"]["from"], "pending");
        assert_eq!(output["statusChange"]["to"], "in_progress");
        assert_eq!(output["revision"], 2);

        let task = tool
            .store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect("task should load")
            .expect("task should exist");
        assert_eq!(task.status, TaskStatus::InProgress);
        assert_eq!(task.owner.as_deref(), Some("agent-a"));
        assert_eq!(task.metadata["priority"], "high");
    }

    #[tokio::test]
    async fn task_update_claims_owner_with_expected_revision() {
        let temp = tempdir().expect("tempdir");
        let tool = tool_update(temp.path());
        tool.store
            .create_task(
                DEFAULT_TASK_LIST_ID,
                NewTaskRecord {
                    subject: "Claim through tool".to_string(),
                    description: "Claim a shared task file.".to_string(),
                    active_form: None,
                    metadata: Default::default(),
                },
            )
            .expect("task should be created");

        let output = tool
            .call(json!({
                "task_id": "1",
                "expectedRevision": 1,
                "claimOwner": "agent-a"
            }))
            .await
            .expect("task_update should claim task");

        assert_eq!(output["success"], true);
        assert_eq!(output["updatedFields"], json!(["owner"]));
        assert_eq!(output["revision"], 2);
        let task = tool
            .store
            .get_task(DEFAULT_TASK_LIST_ID, "1")
            .expect("task should load")
            .expect("task should exist");
        assert_eq!(task.owner.as_deref(), Some("agent-a"));
    }

    #[tokio::test]
    async fn task_update_reports_stale_expected_revision() {
        let temp = tempdir().expect("tempdir");
        let tool = tool_update(temp.path());
        tool.store
            .create_task(
                DEFAULT_TASK_LIST_ID,
                NewTaskRecord {
                    subject: "Reject stale update".to_string(),
                    description: "Reject a stale shared task update.".to_string(),
                    active_form: None,
                    metadata: Default::default(),
                },
            )
            .expect("task should be created");
        tool.store
            .update_task(
                DEFAULT_TASK_LIST_ID,
                "1",
                TaskUpdate {
                    subject: Some("Current subject".to_string()),
                    expected_revision: Some(1),
                    ..TaskUpdate::default()
                },
            )
            .expect("first update should work");

        let output = tool
            .call(json!({
                "task_id": "1",
                "expectedRevision": 1,
                "subject": "Stale subject"
            }))
            .await
            .expect("stale update should return outcome");

        assert_eq!(output["success"], false);
        assert_eq!(output["revision"], 2);
        assert_eq!(output["error"], "Task revision mismatch");
    }

    #[tokio::test]
    async fn task_update_deletes_task_with_deleted_status() {
        let temp = tempdir().expect("tempdir");
        let tool = tool_update(temp.path());
        tool.store
            .create_task(
                DEFAULT_TASK_LIST_ID,
                NewTaskRecord {
                    subject: "Remove obsolete task".to_string(),
                    description: "Delete a shared task file.".to_string(),
                    active_form: None,
                    metadata: Default::default(),
                },
            )
            .expect("task should be created");

        let output = tool
            .call(json!({
                "task_id": "1",
                "status": "deleted"
            }))
            .await
            .expect("task_update should delete task");

        assert_eq!(output["success"], true);
        assert_eq!(output["updatedFields"], json!(["deleted"]));
        assert!(
            tool.store
                .get_task(DEFAULT_TASK_LIST_ID, "1")
                .expect("deleted task read should be valid")
                .is_none()
        );
    }

    #[tokio::test]
    async fn task_update_rejects_alias_conflicts_and_invalid_dependency_ids() {
        let temp = tempdir().expect("tempdir");
        let tool = tool_update(temp.path());

        let alias_err = tool
            .call(json!({
                "taskId": "1",
                "task_id": "1",
                "status": "pending"
            }))
            .await
            .expect_err("mixed task id aliases should fail");
        let dependency_err = tool
            .call(json!({
                "task_id": "1",
                "addBlocks": ["../secret"]
            }))
            .await
            .expect_err("invalid dependency id should fail");

        assert!(alias_err.to_string().contains("either taskId or task_id"));
        assert!(
            dependency_err
                .to_string()
                .contains("invalid task id in addBlocks")
        );
    }

    #[tokio::test]
    async fn task_list_returns_summary_entries() {
        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        fs::create_dir_all(&list_dir).expect("task list dir");
        write_task(
            &list_dir,
            "1",
            json!({
                "id": "1",
                "subject": "Prepare shared store",
                "status": "completed"
            }),
        );
        write_task(
            &list_dir,
            "2",
            json!({
                "id": "2",
                "subject": "Add read tools",
                "status": "pending",
                "owner": "agent-a",
                "blockedBy": ["1", "3"]
            }),
        );

        let tool = TaskListTool {
            store: Arc::new(TaskListStore::new(temp.path())),
            default_task_list_id: DEFAULT_TASK_LIST_ID.to_string(),
        };
        let output = tool.call(json!({})).await.expect("task_list should work");

        assert_eq!(output["tasks"][0]["id"], "1");
        assert_eq!(output["tasks"][1]["owner"], "agent-a");
        assert_eq!(output["tasks"][1]["blockedBy"], json!(["3"]));
    }

    #[tokio::test]
    async fn task_get_returns_full_details_or_null() {
        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        fs::create_dir_all(&list_dir).expect("task list dir");
        write_task(
            &list_dir,
            "2",
            json!({
                "id": "2",
                "subject": "Add read tools",
                "description": "Expose summary and detail reads.",
                "status": "pending",
                "blocks": ["3"],
                "blockedBy": ["1"]
            }),
        );

        let tool = TaskGetTool {
            store: Arc::new(TaskListStore::new(temp.path())),
            default_task_list_id: DEFAULT_TASK_LIST_ID.to_string(),
        };
        let output = tool
            .call(json!({ "task_id": "2" }))
            .await
            .expect("task_get should work");
        let missing = tool
            .call(json!({ "task_id": "missing" }))
            .await
            .expect("missing task should be valid");

        assert_eq!(
            output["task"]["description"],
            "Expose summary and detail reads."
        );
        assert_eq!(output["task"]["blockedBy"], json!(["1"]));
        assert!(missing["task"].is_null());
    }

    #[test]
    fn task_tool_schemas_are_strict() {
        let temp = tempdir().expect("tempdir");
        let task_create_schema = TaskCreateTool::input_schema(&tool_create(temp.path()));
        let task_list_schema = TaskListTool::input_schema(&tool_list());
        let task_update_schema = TaskUpdateTool::input_schema(&tool_update(temp.path()));
        let task_get_schema = TaskGetTool::input_schema(&tool_get());

        assert_eq!(
            task_create_schema.get("additionalProperties"),
            Some(&json!(false))
        );
        assert!(task_create_schema["properties"].get("activeForm").is_some());
        assert!(
            task_create_schema["properties"]
                .get("active_form")
                .is_some()
        );
        assert!(
            task_create_schema["properties"]
                .get("task_list_id")
                .is_some()
        );
        assert!(task_create_schema["properties"].get("taskListId").is_some());
        assert_eq!(
            task_list_schema.get("additionalProperties"),
            Some(&json!(false))
        );
        assert!(task_list_schema["properties"].get("task_list_id").is_some());
        assert!(task_list_schema["properties"].get("taskListId").is_some());
        assert_eq!(
            task_update_schema.get("additionalProperties"),
            Some(&json!(false))
        );
        assert!(task_update_schema["properties"].get("task_id").is_some());
        assert!(task_update_schema["properties"].get("taskId").is_some());
        assert!(
            task_update_schema["properties"]
                .get("expectedRevision")
                .is_some()
        );
        assert!(task_update_schema["properties"].get("claimOwner").is_some());
        assert_eq!(
            task_update_schema.get("oneOf"),
            Some(&json!([
                {"required": ["task_id"]},
                {"required": ["taskId"]}
            ]))
        );
        assert!(task_update_schema["properties"].get("activeForm").is_some());
        assert!(
            task_update_schema["properties"]
                .get("active_form")
                .is_some()
        );
        assert_eq!(
            task_get_schema.get("additionalProperties"),
            Some(&json!(false))
        );
        assert!(task_get_schema["properties"].get("task_list_id").is_some());
        assert!(task_get_schema["properties"].get("taskListId").is_some());
    }

    #[tokio::test]
    async fn task_get_rejects_invalid_task_id() {
        let tool = tool_get();

        for task_id in [" ", "../secret", "nested/task", "nested\\task", "/tmp/task"] {
            let err = tool
                .call(json!({ "task_id": task_id }))
                .await
                .expect_err("invalid task id should fail");

            assert!(err.to_string().contains("without path traversal"));
        }
    }

    fn tool_create(path: &std::path::Path) -> TaskCreateTool {
        TaskCreateTool {
            store: Arc::new(TaskListStore::new(path)),
            default_task_list_id: DEFAULT_TASK_LIST_ID.to_string(),
        }
    }

    fn tool_list() -> TaskListTool {
        let temp = tempdir().expect("tempdir");
        TaskListTool {
            store: Arc::new(TaskListStore::new(temp.path())),
            default_task_list_id: DEFAULT_TASK_LIST_ID.to_string(),
        }
    }

    fn tool_update(path: &std::path::Path) -> TaskUpdateTool {
        TaskUpdateTool {
            store: Arc::new(TaskListStore::new(path)),
            default_task_list_id: DEFAULT_TASK_LIST_ID.to_string(),
        }
    }

    fn tool_get() -> TaskGetTool {
        let temp = tempdir().expect("tempdir");
        TaskGetTool {
            store: Arc::new(TaskListStore::new(temp.path())),
            default_task_list_id: DEFAULT_TASK_LIST_ID.to_string(),
        }
    }

    fn write_task(dir: &std::path::Path, id: &str, task: serde_json::Value) {
        fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec_pretty(&task).expect("serialize task"),
        )
        .expect("write task");
    }
}
