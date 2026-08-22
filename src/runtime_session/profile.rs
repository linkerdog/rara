use std::fmt;
use std::str::FromStr;

use rara_tools::tool::ToolManager;
use serde::{Deserialize, Serialize};

const HEADLESS_CODING_V1_TOOL_NAMES: &[&str] = &[
    "bash",
    "background_task_list",
    "background_task_status",
    "background_task_stop",
    "pty_start",
    "pty_read",
    "pty_list",
    "pty_status",
    "pty_write",
    "pty_kill",
    "pty_stop",
    "read_file",
    "write_file",
    "apply_patch",
    "replace",
    "replace_lines",
    "glob",
    "grep",
    "todo_write",
];

const HEADLESS_CODING_V1_SYSTEM_PROMPT: &str = concat!(
    "Complete the task by acting with the available tools, not by narrating.\n",
    "Inspect the workspace before editing. Prefer read_file, glob, and grep for inspection, ",
    "apply_patch and write_file for file changes, and bash for ordinary commands and tests.\n",
    "Use bash with run_in_background for long-running non-interactive processes and inspect them ",
    "with the background task tools. Use the PTY tools only when terminal input or terminal control is required.\n",
    "Verify the result when practical. Stop when the task is complete."
);

/// Selects a stable runtime composition for one session.
///
/// Versioned profiles freeze the model-visible prompt and tool surface. Hosts
/// selecting a profile must treat it as an upper bound: tools outside that
/// profile are removed even when the host supplies a custom tool manager.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeSessionProfile {
    /// Use the configured application runtime without additional projection.
    #[default]
    Default,
    /// Use the reproducible non-interactive coding surface used by harnesses.
    HeadlessCodingV1,
}

impl RuntimeSessionProfile {
    /// Stable profile name used by CLI and trajectory metadata.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::HeadlessCodingV1 => "headless-coding-v1",
        }
    }

    /// Exact tool allowlist for versioned profiles.
    ///
    /// `None` means the default application registry is not projected.
    pub const fn tool_names(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Default => None,
            Self::HeadlessCodingV1 => Some(HEADLESS_CODING_V1_TOOL_NAMES),
        }
    }

    pub(crate) const fn disables_ambient_facilities(self) -> bool {
        match self {
            Self::Default => false,
            Self::HeadlessCodingV1 => true,
        }
    }

    pub(crate) const fn system_prompt(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::HeadlessCodingV1 => Some(HEADLESS_CODING_V1_SYSTEM_PROMPT),
        }
    }

    pub(crate) fn project_tools(self, tools: &mut ToolManager) {
        let Some(allowed) = self.tool_names() else {
            return;
        };
        tools.retain(|name| allowed.contains(&name));
    }
}

impl fmt::Display for RuntimeSessionProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RuntimeSessionProfile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "default" => Ok(Self::Default),
            "headless-coding-v1" => Ok(Self::HeadlessCodingV1),
            other => Err(format!(
                "unknown runtime profile '{other}'; expected default or headless-coding-v1"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use async_trait::async_trait;
    use rara_tools::tool::{Tool, ToolError};
    use serde_json::{Value, json};

    use super::*;

    struct NamedTool(&'static str);

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> &str {
            "test tool"
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn call(&self, _input: Value) -> Result<Value, ToolError> {
            Ok(json!({}))
        }
    }

    #[test]
    fn headless_profile_names_are_unique_and_round_trip() {
        let names = RuntimeSessionProfile::HeadlessCodingV1
            .tool_names()
            .expect("headless tool names");
        assert_eq!(names.len(), names.iter().collect::<BTreeSet<_>>().len());
        assert_eq!(
            "headless-coding-v1".parse(),
            Ok(RuntimeSessionProfile::HeadlessCodingV1)
        );
        assert_eq!(
            RuntimeSessionProfile::HeadlessCodingV1.to_string(),
            "headless-coding-v1"
        );
    }

    #[test]
    fn headless_profile_is_an_upper_bound_for_custom_tools() {
        let mut tools = ToolManager::new();
        tools.register(Box::new(NamedTool("bash")));
        tools.register(Box::new(NamedTool("host_private_tool")));

        RuntimeSessionProfile::HeadlessCodingV1.project_tools(&mut tools);

        assert!(tools.get_tool("bash").is_some());
        assert!(tools.get_tool("host_private_tool").is_none());
    }
}
