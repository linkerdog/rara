/// Auto-permission classifier types and transcript projection helpers.
///
/// Mirrors Claude Code's yoloClassifier approach:
/// - Excludes assistant reasoning text from classifier input
/// - Only includes user messages and structured tool-call projections
/// - Static deny rules always override classifier allow decisions
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Auto Permission Classifier ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoPermissionRequest {
    /// The tool name being invoked (e.g. "bash", "web_search")
    pub tool_name: String,
    /// Structured tool input projection (only relevant fields)
    pub tool_input: Value,
    /// Optional workspace context hint (e.g. current directory)
    pub workspace_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoPermissionDecision {
    /// Allow the operation, proceed without user approval
    Allow,
    /// Deny the operation, do not execute
    Deny,
    /// Ask the user for explicit approval
    Ask,
}

impl std::fmt::Display for AutoPermissionDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allow => write!(f, "allow"),
            Self::Deny => write!(f, "deny"),
            Self::Ask => write!(f, "ask"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoPermissionResponse {
    pub decision: AutoPermissionDecision,
    /// Human-readable reason for the decision
    pub reason: String,
    /// Optional matched policy rule name
    pub matched_rule: Option<String>,
}

// ── Background Task Status Classifier ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTaskClassifyRequest {
    /// The command that was run
    pub command: String,
    /// The task status: running, completed, killed
    pub status: String,
    /// Tail of the task's output (last ~2KB)
    pub output_tail: String,
    /// How long the task has been running
    pub elapsed: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskState {
    /// Task is actively producing output
    Working,
    /// Task appears stuck waiting for something
    Blocked,
    /// Task completed successfully
    Done,
    /// Task failed with an error
    Failed,
}

impl std::fmt::Display for BackgroundTaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Working => write!(f, "working"),
            Self::Blocked => write!(f, "blocked"),
            Self::Done => write!(f, "done"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskTempo {
    Active,
    Idle,
    Blocked,
}

impl std::fmt::Display for BackgroundTaskTempo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Idle => write!(f, "idle"),
            Self::Blocked => write!(f, "blocked"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTaskClassifyResponse {
    pub state: BackgroundTaskState,
    pub tempo: BackgroundTaskTempo,
    /// One-line status description
    pub detail: String,
    /// What the user needs to do to unblock (only filled when Blocked)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}

// ── Transcript Projection Helpers ──────────────────────────────────────────────

/// Build a classifier prompt from the conversation messages, excluding assistant
/// reasoning text blocks. Only user messages and structured tool-call content
/// are included — mirroring Claude Code's approach.
pub fn build_classifier_messages(
    messages: &[crate::agent::Message],
    current_tool_name: &str,
    current_tool_input: &Value,
) -> Vec<crate::agent::Message> {
    use crate::agent::Message;
    let mut result: Vec<Message> = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "user" => {
                // Only include user text content, not tool results
                if let Some(text) = extract_text_content(&msg.content)
                    && !text.is_empty()
                {
                    result.push(Message {
                        role: "user".to_string(),
                        content: Value::String(text),
                    });
                }
            }
            "assistant" => {
                // Only include assistant tool_use blocks, exclude text reasoning
                if let Some(items) = msg.content.as_array() {
                    let tool_calls: Vec<String> = items
                        .iter()
                        .filter_map(|item| {
                            if item.get("type").and_then(Value::as_str) == Some("tool_use") {
                                let name = item.get("name").and_then(Value::as_str).unwrap_or("?");
                                let input = item
                                    .get("input")
                                    .map(|v| serde_json::to_string(v).unwrap_or_default())
                                    .unwrap_or_default();
                                Some(format!("tool_call {}: {}", name, input))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !tool_calls.is_empty() {
                        result.push(Message {
                            role: "assistant".to_string(),
                            content: Value::String(tool_calls.join("\n")),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    // Append the current tool call being evaluated
    result.push(Message {
        role: "user".to_string(),
        content: Value::String(format!(
            "Evaluate this tool call: {} {}",
            current_tool_name,
            serde_json::to_string(current_tool_input).unwrap_or_default()
        )),
    });

    result
}

/// Build a classifier message for background task status evaluation.
pub fn build_background_task_message(
    request: &BackgroundTaskClassifyRequest,
) -> crate::agent::Message {
    let mut parts = vec![
        format!("Command: {}", request.command),
        format!("Status: {}", request.status),
    ];

    if let Some(ref elapsed) = request.elapsed {
        parts.push(format!("Elapsed: {}", elapsed));
    }

    if !request.output_tail.is_empty() {
        parts.push(format!("Recent output:\n{}", request.output_tail));
    }

    crate::agent::Message {
        role: "user".into(),
        content: Value::String(parts.join("\n")),
    }
}

fn extract_text_content(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    if let Some(items) = content.as_array() {
        let texts: Vec<String> = items
            .iter()
            .filter_map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    item.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect();
        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }
    None
}

/// Parse AutoPermissionResponse from classifier LLM output.
pub fn parse_auto_permission_response(
    raw: &str,
) -> Result<AutoPermissionResponse, serde_json::Error> {
    if let Ok(resp) = serde_json::from_str::<AutoPermissionResponse>(raw) {
        return Ok(resp);
    }
    serde_json::from_str(clean_json_response(raw))
}

/// Parse BackgroundTaskClassifyResponse from classifier LLM output.
pub fn parse_background_task_response(
    raw: &str,
) -> Result<BackgroundTaskClassifyResponse, serde_json::Error> {
    if let Ok(resp) = serde_json::from_str::<BackgroundTaskClassifyResponse>(raw) {
        return Ok(resp);
    }
    serde_json::from_str(clean_json_response(raw))
}

/// Strip surrounding markdown code fences and whitespace from LLM JSON output.
fn clean_json_response(raw: &str) -> &str {
    raw.trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::Message;

    #[test]
    fn test_build_classifier_messages_excludes_assistant_text() {
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: json!("run cargo build"),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type": "text", "text": "I'll run the build for you."},
                    {"type": "tool_use", "name": "bash", "id": "call_1", "input": {"command": "cargo build"}}
                ]),
            },
        ];

        let result = build_classifier_messages(&messages, "bash", &json!({"command": "rm -rf /"}));

        // Must include user message
        let user_msg = &result[0];
        assert_eq!(user_msg.role, "user");
        assert!(
            user_msg
                .content
                .as_str()
                .unwrap()
                .contains("run cargo build")
        );

        // Must NOT include assistant text reasoning
        let assistant_msg = &result[1];
        assert_eq!(assistant_msg.role, "assistant");
        let assistant_content = assistant_msg.content.as_str().unwrap();
        assert!(!assistant_content.contains("I'll run the build for you"));
        assert!(assistant_content.contains("bash"));

        // Must include the current (dangerous) tool call
        let eval_msg = &result[2];
        assert!(eval_msg.content.as_str().unwrap().contains("rm -rf /"));
    }

    #[test]
    fn test_parse_auto_permission_allow() {
        let raw = r#"{"decision": "allow", "reason": "Safe read-only operation"}"#;
        let resp = parse_auto_permission_response(raw).unwrap();
        assert_eq!(resp.decision, AutoPermissionDecision::Allow);
        assert_eq!(resp.reason, "Safe read-only operation");
    }

    #[test]
    fn test_parse_auto_permission_deny() {
        let raw = r#"{"decision": "deny", "reason": "Destructive filesystem operation"}"#;
        let resp = parse_auto_permission_response(raw).unwrap();
        assert_eq!(resp.decision, AutoPermissionDecision::Deny);
        assert_eq!(resp.reason, "Destructive filesystem operation");
    }

    #[test]
    fn test_parse_auto_permission_code_block() {
        let raw = "```json\n{\"decision\": \"ask\", \"reason\": \"network request\"}\n```";
        let resp = parse_auto_permission_response(raw).unwrap();
        assert_eq!(resp.decision, AutoPermissionDecision::Ask);
    }

    #[test]
    fn test_parse_background_task_working() {
        let raw = r#"{"state": "working", "tempo": "active", "detail": "Building in progress"}"#;
        let resp = parse_background_task_response(raw).unwrap();
        assert_eq!(resp.state, BackgroundTaskState::Working);
        assert_eq!(resp.tempo, BackgroundTaskTempo::Active);
    }

    #[test]
    fn test_parse_background_task_blocked() {
        let raw = r#"{"state": "blocked", "tempo": "blocked", "detail": "Waiting for input", "needs": "Provide SSH passphrase"}"#;
        let resp = parse_background_task_response(raw).unwrap();
        assert_eq!(resp.state, BackgroundTaskState::Blocked);
        assert_eq!(resp.tempo, BackgroundTaskTempo::Blocked);
        assert_eq!(resp.needs, Some("Provide SSH passphrase".to_string()));
    }

    #[test]
    fn test_build_background_task_message() {
        let request = BackgroundTaskClassifyRequest {
            command: "cargo build --release".to_string(),
            status: "running".to_string(),
            output_tail: "   Compiling rara v0.0.1\n   Compiling ...".to_string(),
            elapsed: Some("30s".to_string()),
        };

        let msg = build_background_task_message(&request);
        let prompt = msg.content.as_str().unwrap();
        assert!(prompt.contains("cargo build --release"));
        assert!(prompt.contains("running"));
        assert!(prompt.contains("30s"));
        assert!(prompt.contains("Compiling rara"));
    }
}
