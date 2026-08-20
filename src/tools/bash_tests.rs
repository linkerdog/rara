use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use rara_background_tasks::{
    BackgroundTaskListTool, BackgroundTaskStatus, BackgroundTaskStatusTool, BackgroundTaskStopTool,
    BackgroundTaskStore, BashStreamKind, read_output_tail,
};
use rara_tools::tool::{Tool, ToolCallContext, ToolOutputStream, ToolProgressEvent};
use serde_json::{Value, json};
use tempfile::tempdir;

use super::{
    BashCommandInput, BashSandboxPermissions, BashTool, append_aggregated_bash_output,
    command_env_for_wrapped, sandbox_command_env, sandbox_output_hint,
    unsandboxed_execution_warning,
};
use crate::sandbox::{SandboxManager, WrappedCommand};
use crate::tool_result::model_preview_bash_output;

#[test]
fn parses_legacy_shell_payload() {
    let input = BashCommandInput::from_value(json!({
        "command": "cargo test",
        "allow_net": true
    }))
    .expect("legacy payload");

    assert_eq!(input.command.as_deref(), Some("cargo test"));
    assert!(input.allow_net);
    assert!(!input.run_in_background);
    assert_eq!(input.summary(), "cargo test");
}

#[test]
fn normalizes_simple_absolute_cd_prefix() {
    let input = BashCommandInput::from_value(json!({
        "command": "cd /tmp/workspace && cargo check",
    }))
    .expect("legacy payload");

    assert_eq!(input.cwd.as_deref(), Some("/tmp/workspace"));
    assert_eq!(input.command.as_deref(), Some("cargo check"));
    assert_eq!(input.summary(), "cargo check");
}

#[test]
fn normalizes_simple_quoted_absolute_cd_prefix() {
    let input = BashCommandInput::from_value(json!({
        "command": "cd '/tmp/work space' && cargo test",
    }))
    .expect("legacy payload");

    assert_eq!(input.cwd.as_deref(), Some("/tmp/work space"));
    assert_eq!(input.command.as_deref(), Some("cargo test"));
}

#[test]
fn leaves_complex_or_ambiguous_cd_prefix_unchanged() {
    let relative = BashCommandInput::from_value(json!({
        "command": "cd crates && cargo check",
    }))
    .expect("relative payload");
    assert_eq!(relative.cwd, None);
    assert_eq!(
        relative.command.as_deref(),
        Some("cd crates && cargo check")
    );

    let existing_cwd = BashCommandInput::from_value(json!({
        "command": "cd /tmp/other && cargo check",
        "cwd": "/tmp/workspace",
    }))
    .expect("cwd payload");
    assert_eq!(existing_cwd.cwd.as_deref(), Some("/tmp/workspace"));
    assert_eq!(
        existing_cwd.command.as_deref(),
        Some("cd /tmp/other && cargo check")
    );
}

#[test]
fn parses_structured_payload() {
    let input = BashCommandInput::from_value(json!({
        "program": "cargo",
        "args": ["check", "--workspace"],
        "cwd": "/tmp/workspace",
        "env": { "RUST_LOG": "debug" },
        "allow_net": false
    }))
    .expect("structured payload");

    assert_eq!(input.program.as_deref(), Some("cargo"));
    assert_eq!(
        input.args,
        vec!["check".to_string(), "--workspace".to_string()]
    );
    assert_eq!(input.cwd.as_deref(), Some("/tmp/workspace"));
    assert_eq!(input.env.get("RUST_LOG").map(String::as_str), Some("debug"));
    assert!(!input.run_in_background);
    assert_eq!(input.summary(), "cargo check --workspace");
}

#[test]
fn parses_background_payload() {
    let input = BashCommandInput::from_value(json!({
        "program": "cargo",
        "args": ["test"],
        "run_in_background": true
    }))
    .expect("background payload");

    assert!(input.run_in_background);
    assert_eq!(input.summary(), "cargo test");
}

#[test]
fn parses_codex_style_escalated_sandbox_request() {
    let input = BashCommandInput::from_value(json!({
        "program": "cargo",
        "args": ["check"],
        "sandbox_permissions": "require_escalated",
        "justification": "Do you want to run cargo check outside the sandbox?",
        "prefix_rule": ["cargo", "check"]
    }))
    .expect("escalated payload");

    assert_eq!(
        input.sandbox_permissions,
        BashSandboxPermissions::RequireEscalated
    );
    assert_eq!(
        input.justification.as_deref(),
        Some("Do you want to run cargo check outside the sandbox?")
    );
    assert_eq!(input.approval_prefix().as_deref(), Some("cargo check"));
    assert!(!input.is_read_only());
}

#[test]
fn bash_tool_schema_guides_command_discipline() {
    let temp = tempdir().expect("tempdir");
    let tool = BashTool {
        sandbox: Arc::new(
            SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox"),
        ),
        background_tasks: Arc::new(
            BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                .expect("background task store"),
        ),
        base_env: Arc::new(HashMap::new()),
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
    };

    let description = tool.description();
    assert!(description.contains("Prefer dedicated RARA tools"));
    assert!(description.contains("Edit files with apply_patch"));
    assert!(description.contains("replace_lines"));
    assert!(description.contains("sed -i"));
    assert!(description.contains("apply_patch"));
    assert!(description.contains("cwd field"));
    assert!(description.contains("newline-separated command chaining"));
    assert!(description.contains("Commands must be non-interactive"));
    assert!(description.contains("git commit -m"));
    assert!(description.contains("require_escalated"));
    assert!(description.contains("do not stop verification"));
    assert!(
        description.contains("Do not re-run the exact same denied sandboxed validation command")
    );
    assert!(description.contains("background_task_status"));

    let schema = tool.input_schema().to_string();
    assert!(schema.contains("Prefer program+args"));
    assert!(schema.contains("Do not prefix this command with cd"));
    assert!(schema.contains("apply_patch, replace, replace_lines"));
    assert!(schema.contains("never bare git commit"));
    assert!(schema.contains("request escalated permissions instead of giving up"));
    assert!(schema.contains("Do not repeat the exact same denied sandboxed validation call"));
    assert!(schema.contains("Use this instead of prefixing the command with cd"));
    assert!(schema.contains("sandbox failure evidence"));
    assert!(schema.contains("prefer require_escalated over repeating the denied sandboxed call"));
    assert!(schema.contains("Do not suggest broad prefixes"));
}

#[test]
fn background_task_tool_descriptions_point_to_run_in_background() {
    let temp = tempdir().expect("tempdir");
    let background_tasks = Arc::new(
        BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
            .expect("background task store"),
    );
    let list = BackgroundTaskListTool {
        background_tasks: background_tasks.clone(),
    };
    let status = BackgroundTaskStatusTool {
        background_tasks: background_tasks.clone(),
    };
    let stop = BackgroundTaskStopTool { background_tasks };

    assert!(list.description().contains("run_in_background"));
    assert!(list.description().contains("duplicate long-running work"));
    assert!(status.description().contains("run_in_background"));
    assert!(stop.description().contains("task_id is omitted"));
}

#[test]
fn escalated_sandbox_request_allows_missing_justification() {
    let input = BashCommandInput::from_value(json!({
        "program": "cargo",
        "args": ["check"],
        "sandbox_permissions": "require_escalated"
    }))
    .expect("escalated payload");

    assert_eq!(
        input.sandbox_permissions,
        BashSandboxPermissions::RequireEscalated
    );
    assert!(input.justification.is_none());
    assert!(!input.is_read_only());
}

#[test]
fn classifies_read_only_commands_for_approval_policy() {
    for command in [
        "git status --short",
        "git diff -- src/tools/bash.rs",
        "git log --oneline -n 5",
        "rg -n read_only src",
        "find src -name '*.rs'",
        "sed -n '1,20p' src/tools/bash.rs",
        "cat Cargo.toml | grep '^name'",
        "docker inspect rara-dev",
        "pyright --outputjson",
    ] {
        let input =
            BashCommandInput::from_value(json!({ "command": command })).expect("bash payload");
        assert!(input.is_read_only(), "{command} should be read-only");
    }
}

#[test]
fn keeps_write_network_background_and_complex_commands_under_approval() {
    for payload in [
        json!({ "command": "git push origin main" }),
        json!({ "command": "rm -rf target" }),
        json!({ "command": "sed -i '' 's/a/b/' Cargo.toml" }),
        json!({ "command": "find . -name '*.tmp' -delete" }),
        json!({ "command": "cat Cargo.toml > /tmp/out" }),
        json!({ "command": "git status", "allow_net": true }),
        json!({ "command": "rg TODO", "run_in_background": true }),
        json!({ "program": "rg", "args": ["TODO"], "env": { "PATH": "/tmp/bin" } }),
    ] {
        let input = BashCommandInput::from_value(payload).expect("bash payload");
        assert!(
            !input.is_read_only(),
            "{} should require approval",
            input.summary()
        );
    }
}

#[test]
fn classifies_structured_read_only_programs() {
    let input = BashCommandInput::from_value(json!({
        "program": "/usr/bin/git",
        "args": ["status", "--short"]
    }))
    .expect("structured payload");

    assert!(input.is_read_only());
}

#[test]
fn derives_and_matches_codex_style_approval_prefix() {
    let input = BashCommandInput::from_value(json!({
        "command": "git push origin main"
    }))
    .expect("bash payload");

    assert_eq!(input.approval_prefix().as_deref(), Some("git push"));
    assert!(input.matches_approval_prefix("git push"));
    assert!(!input.matches_approval_prefix("git pull"));
}

#[test]
fn approval_prefix_matching_normalizes_program_paths() {
    let shell_input = BashCommandInput::from_value(json!({
        "command": "/usr/bin/git push origin main"
    }))
    .expect("shell payload");
    assert_eq!(shell_input.approval_prefix().as_deref(), Some("git push"));
    assert!(shell_input.matches_approval_prefix("git push"));

    let structured_input = BashCommandInput::from_value(json!({
        "program": "/usr/bin/git",
        "args": ["push", "origin", "main"]
    }))
    .expect("structured payload");
    assert_eq!(
        structured_input.approval_prefix().as_deref(),
        Some("git push")
    );
    assert!(structured_input.matches_approval_prefix("git push"));
}

#[test]
fn approval_prefix_skips_known_global_options() {
    let input = BashCommandInput::from_value(json!({
        "command": "git --no-pager push origin main"
    }))
    .expect("shell payload");

    assert_eq!(input.approval_prefix().as_deref(), Some("git push"));
    assert!(input.matches_approval_prefix("git push"));
}

#[test]
fn approval_prefix_does_not_match_multi_segment_command_by_first_segment() {
    let input = BashCommandInput::from_value(json!({
        "command": "git push origin main && rm -rf target"
    }))
    .expect("shell payload");

    assert_eq!(input.approval_prefix(), None);
    assert!(!input.matches_approval_prefix("git push"));
    assert!(!input.is_allowed_by_approval_prefixes(&["git push".to_string()]));
}

#[test]
fn approval_prefixes_evaluate_every_shell_segment() {
    let push_then_status = BashCommandInput::from_value(json!({
        "command": "git push origin main && git status --short"
    }))
    .expect("shell payload");
    assert!(
        push_then_status.is_allowed_by_approval_prefixes(&["git push".to_string()]),
        "approved write segment plus read-only segment should be allowed"
    );

    let push_then_test = BashCommandInput::from_value(json!({
        "command": "git push origin main && cargo test"
    }))
    .expect("shell payload");
    assert!(!push_then_test.is_allowed_by_approval_prefixes(&["git push".to_string()]));
    assert!(
        push_then_test
            .is_allowed_by_approval_prefixes(&["git push".to_string(), "cargo test".to_string(),])
    );
}

#[test]
fn approval_prefix_fallback_allows_only_exact_multi_segment_command() {
    let input = BashCommandInput::from_value(json!({
        "command": "git push origin main && git status --short"
    }))
    .expect("shell payload");
    let exact_approval = "git push origin main && git status --short".to_string();

    assert_eq!(input.approval_prefix(), None);
    assert!(input.is_allowed_by_approval_prefixes(std::slice::from_ref(&exact_approval)));

    let extra_segment = BashCommandInput::from_value(json!({
        "command": "git push origin main && git status --short && rm -rf target"
    }))
    .expect("shell payload");
    assert!(!extra_segment.is_allowed_by_approval_prefixes(&[exact_approval]));
}

#[test]
fn approval_prefixes_reject_shell_features_outside_rule_matching() {
    for command in [
        "git push origin main > /tmp/out",
        "git push origin $BRANCH",
        "git push origin main && rm -rf *",
        "FOO=bar git push origin main",
        "(git push origin main)",
    ] {
        let input =
            BashCommandInput::from_value(json!({ "command": command })).expect("shell payload");
        assert_eq!(
            input.approval_prefix(),
            None,
            "{command} should not suggest a reusable prefix"
        );
        assert!(
            !input.is_allowed_by_approval_prefixes(&["git push".to_string()]),
            "{command} should not be allowed by a simple prefix"
        );
    }
}

#[test]
fn sandbox_command_env_defaults_home_and_xdg_roots() {
    let sandbox_home = Path::new("/tmp/rara-test-home");
    let base_env = HashMap::from([("PATH".to_string(), "/custom/bin:/usr/bin".to_string())]);
    let env_map = sandbox_command_env(sandbox_home, &base_env, &HashMap::new(), true);

    assert_eq!(
        env_map.get("HOME").map(String::as_str),
        Some("/tmp/rara-test-home")
    );
    assert_eq!(
        env_map.get("XDG_CONFIG_HOME").map(String::as_str),
        Some("/tmp/rara-test-home/.config")
    );
    assert_eq!(
        env_map.get("XDG_CACHE_HOME").map(String::as_str),
        Some("/tmp/rara-test-home/.cache")
    );
    assert_eq!(
        env_map.get("PATH").map(String::as_str),
        Some("/custom/bin:/usr/bin")
    );
}

#[test]
fn sandbox_command_env_keeps_explicit_overrides() {
    let sandbox_home = Path::new("/tmp/rara-test-home");
    let env_map = sandbox_command_env(
        sandbox_home,
        &HashMap::from([("PATH".to_string(), "/snapshot/bin".to_string())]),
        &HashMap::from([
            ("HOME".to_string(), "/custom/home".to_string()),
            (
                "XDG_CACHE_HOME".to_string(),
                "/custom/home/.cache".to_string(),
            ),
            ("PATH".to_string(), "/override/bin".to_string()),
        ]),
        true,
    );

    assert_eq!(
        env_map.get("HOME").map(String::as_str),
        Some("/custom/home")
    );
    assert_eq!(
        env_map.get("XDG_CACHE_HOME").map(String::as_str),
        Some("/custom/home/.cache")
    );
    assert_eq!(
        env_map.get("XDG_CONFIG_HOME").map(String::as_str),
        Some("/tmp/rara-test-home/.config")
    );
    assert_eq!(
        env_map.get("PATH").map(String::as_str),
        Some("/override/bin")
    );
}

#[test]
fn sandbox_command_env_falls_back_to_process_path_when_snapshot_path_is_missing() {
    let sandbox_home = Path::new("/tmp/rara-test-home");
    let env_map = sandbox_command_env(
        sandbox_home,
        &HashMap::from([("PATH".to_string(), String::new())]),
        &HashMap::new(),
        true,
    );

    assert!(
        env_map.get("PATH").is_some_and(|path| !path.is_empty()),
        "sandbox env must keep a usable PATH after env_clear"
    );
}

#[test]
fn sandbox_command_env_marks_disabled_network() {
    let env_map = sandbox_command_env(
        Path::new("/tmp/rara-test-home"),
        &HashMap::new(),
        &HashMap::new(),
        false,
    );

    assert_eq!(
        env_map
            .get("RARA_SANDBOX_NETWORK_DISABLED")
            .map(String::as_str),
        Some("1")
    );
}

#[test]
fn sandbox_output_hint_explains_blocked_shell_paths() {
    let hint = sandbox_output_hint("sandbox-exec: /bin/sed: Operation not permitted")
        .expect("sandbox hint");

    assert!(hint.contains("Prefer direct file tools"));
    assert!(hint.contains("replace_lines"));
}

#[test]
fn direct_wrapped_command_keeps_caller_environment_overrides_only() {
    let wrapped = WrappedCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "pwd".to_string()],
        cleanup_path: None,
        sandboxed: false,
        sandbox_backend: "direct".to_string(),
        sandbox_home: None,
        network_access: true,
    };
    let env_map = command_env_for_wrapped(
        &wrapped,
        &HashMap::from([("PATH".to_string(), "/snapshot/bin".to_string())]),
        &HashMap::from([("HOME".to_string(), "/real/home".to_string())]),
    )
    .expect("direct env");

    assert_eq!(env_map.get("HOME").map(String::as_str), Some("/real/home"));
    assert_eq!(
        env_map.get("PATH").map(String::as_str),
        Some("/snapshot/bin")
    );
    assert!(
        !env_map.contains_key("XDG_CONFIG_HOME"),
        "direct fallback should not apply sandbox-only XDG roots"
    );
}

#[test]
fn unsandboxed_warning_names_the_backend() {
    let wrapped = WrappedCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "pwd".to_string()],
        cleanup_path: None,
        sandboxed: false,
        sandbox_backend: "direct".to_string(),
        sandbox_home: None,
        network_access: true,
    };

    let warning = unsandboxed_execution_warning(&wrapped);

    assert!(warning.contains("without sandbox isolation"));
    assert!(warning.contains("direct"));
}

#[tokio::test]
async fn escalated_sandbox_request_runs_directly_after_approval() {
    let temp = tempdir().expect("tempdir");
    let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
    let tool = BashTool {
        sandbox: Arc::new(sandbox),
        background_tasks: Arc::new(
            BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                .expect("background task store"),
        ),
        base_env: Arc::new(HashMap::new()),
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
    };

    let result = tool
        .call(json!({
            "program": "sh",
            "args": ["-c", "printf direct"],
            "sandbox_permissions": "require_escalated",
            "justification": "Do you want to run this shell outside the sandbox?"
        }))
        .await
        .expect("bash result");

    assert_eq!(
        result.get("sandboxed").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        result.get("sandbox_backend").and_then(Value::as_str),
        Some("direct")
    );
    assert_eq!(result.get("stdout").and_then(Value::as_str), Some("direct"));
    // RequireEscalated explicitly bypasses sandbox isolation — no warning needed.
    let aggregated_output = result
        .get("aggregated_output")
        .and_then(Value::as_str)
        .expect("aggregated output");
    assert!(aggregated_output.contains("direct"));
    assert!(
        !aggregated_output.contains("without sandbox isolation"),
        "RequireEscalated should suppress the sandbox warning"
    );
    assert!(result.get("duration_ms").and_then(Value::as_u64).is_some());
    assert!(
        result
            .get("stderr")
            .and_then(Value::as_str)
            .is_none_or(|stderr| !stderr.contains("without sandbox isolation"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn foreground_signal_is_reported_as_structured_termination() {
    let temp = tempdir().expect("tempdir");
    let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
    let tool = BashTool {
        sandbox: Arc::new(sandbox),
        background_tasks: Arc::new(
            BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                .expect("background task store"),
        ),
        base_env: Arc::new(HashMap::new()),
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
    };

    let result = tool
        .call(json!({
            "program": "/bin/sh",
            "args": ["-c", "kill -ABRT $$"],
            "sandbox_permissions": "require_escalated"
        }))
        .await
        .expect("bash signal result");

    assert_eq!(result.get("exit_code"), Some(&Value::Null));
    assert_eq!(result["termination"]["kind"], "signal");
    assert_eq!(result["termination"]["signal"], libc::SIGABRT);
    assert_eq!(result["termination"]["name"], "SIGABRT");
    assert!(result.get("sandbox_failure").is_none());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn sandbox_policy_denial_is_machine_readable() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).expect("workspace");
    let denied_path = temp.path().join("outside-workspace.txt");
    let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
    let tool = BashTool {
        sandbox: Arc::new(sandbox),
        background_tasks: Arc::new(
            BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                .expect("background task store"),
        ),
        base_env: Arc::new(HashMap::new()),
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
    };

    let result = tool
        .call(json!({
            "program": "/usr/bin/touch",
            "args": [denied_path],
            "cwd": workspace,
        }))
        .await
        .expect("bash denial result");

    assert_eq!(result["termination"]["kind"], "exit");
    assert_ne!(result["exit_code"], 0);
    assert_eq!(result["sandbox_failure"]["kind"], "policy_denied");
    assert_eq!(result["sandbox_failure"]["backend"], "macos-seatbelt");
    assert!(!denied_path.exists(), "sandbox must contain writes");
}

#[tokio::test]
async fn streaming_call_reports_stdout_and_stderr_chunks() {
    let temp = tempdir().expect("tempdir");
    let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
    let Ok(wrapped) = sandbox.wrap_exec_command(
        "/bin/sh",
        &[
            "-c".to_string(),
            "printf 'out\\n'; printf 'err\\n' >&2".to_string(),
        ],
        temp.path().to_string_lossy().as_ref(),
        false,
    ) else {
        return;
    };
    if !binary_exists(&wrapped.program) {
        return;
    }
    // Streaming through some sandbox backends (e.g. macOS seatbelt)
    // is not supported; skip this test under sandboxed execution.
    if wrapped.sandboxed {
        return;
    }
    let tool = BashTool {
        sandbox: Arc::new(sandbox),
        background_tasks: Arc::new(
            BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                .expect("background task store"),
        ),
        base_env: Arc::new(HashMap::new()),
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
    };
    let mut events = Vec::new();
    let result = tool
        .call_with_events(
            json!({
                "program": "/bin/sh",
                "args": ["-c", "printf 'out\\n'; printf 'err\\n' >&2"],
            }),
            &mut |event| events.push(event),
        )
        .await
        .expect("bash result");

    assert!(
        !events.is_empty(),
        "expected streamed events, got result: {result}"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        ToolProgressEvent::Output {
            stream: ToolOutputStream::Stdout | ToolOutputStream::Stderr,
            ..
        }
    )));
    assert_eq!(
        result.get("live_streamed").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        result.get("sandboxed").and_then(Value::as_bool),
        Some(wrapped.sandboxed)
    );
    assert_eq!(
        result.get("sandbox_backend").and_then(Value::as_str),
        Some(wrapped.sandbox_backend.as_str())
    );
    let aggregated_output = result
        .get("aggregated_output")
        .and_then(Value::as_str)
        .expect("aggregated output");
    assert!(aggregated_output.contains("out"));
    assert!(aggregated_output.contains("[stderr] err"));
    let model_preview_output = result
        .get("model_preview_output")
        .and_then(Value::as_str)
        .expect("model preview output");
    assert!(model_preview_output.contains("out"));
    assert!(model_preview_output.contains("[stderr] err"));
    assert!(result.get("duration_ms").and_then(Value::as_u64).is_some());
}

#[tokio::test]
async fn foreground_bash_can_be_cancelled() {
    let temp = tempdir().expect("tempdir");
    let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
    let tool = BashTool {
        sandbox: Arc::new(sandbox),
        background_tasks: Arc::new(
            BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                .expect("background task store"),
        ),
        base_env: Arc::new(HashMap::new()),
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
    };
    let cancellation = Arc::new(AtomicBool::new(false));
    let cancellation_for_task = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancellation_for_task.store(true, Ordering::SeqCst);
    });
    let mut events = Vec::new();

    let err = tool
        .call_with_context_events(
            json!({
                "program": "sh",
                "args": ["-c", "sleep 30 & wait"],
                "sandbox_permissions": "require_escalated"
            }),
            ToolCallContext::default().with_cancellation(cancellation),
            &mut |event| events.push(event),
        )
        .await
        .expect_err("bash should be cancelled");

    assert!(err.to_string().contains("cancelled by user"));
    assert!(events.iter().any(|event| matches!(
        event,
        ToolProgressEvent::Output {
            stream: ToolOutputStream::Stderr,
            chunk,
        } if chunk.contains("cancelled by user")
    )));
}

#[test]
fn aggregated_stderr_prefixes_only_line_boundaries() {
    let mut output = String::new();
    let mut last_stream = None;
    append_aggregated_bash_output(
        &mut output,
        &mut last_stream,
        BashStreamKind::Stderr,
        "partial",
    );
    append_aggregated_bash_output(
        &mut output,
        &mut last_stream,
        BashStreamKind::Stderr,
        "-line\nnext",
    );
    append_aggregated_bash_output(
        &mut output,
        &mut last_stream,
        BashStreamKind::Stderr,
        "-line\n",
    );

    assert_eq!(output, "[stderr] partial-line\n[stderr] next-line\n");
}

#[test]
fn aggregated_stderr_starts_on_new_line_after_stdout() {
    let mut output = String::new();
    let mut last_stream = None;
    append_aggregated_bash_output(
        &mut output,
        &mut last_stream,
        BashStreamKind::Stdout,
        "stdout-without-newline",
    );
    append_aggregated_bash_output(
        &mut output,
        &mut last_stream,
        BashStreamKind::Stderr,
        "stderr-line\n",
    );

    assert_eq!(output, "stdout-without-newline\n[stderr] stderr-line\n");
}

#[test]
fn model_preview_bash_output_preserves_error_tail() {
    let output = format!("head\n{}tail-error\n", "middle\n".repeat(2_000));

    let preview = model_preview_bash_output(&output, Some(1));

    assert!(preview.contains("head"));
    assert!(preview.contains("tail-error"));
    assert!(preview.contains("chars truncated from middle"));
}

#[tokio::test]
async fn background_call_returns_task_and_status_reads_output() {
    let temp = tempdir().expect("tempdir");
    let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
    let Ok(wrapped) = sandbox.wrap_exec_command(
        "sh",
        &["-c".to_string(), "printf 'background-out\\n'".to_string()],
        temp.path().to_string_lossy().as_ref(),
        false,
    ) else {
        return;
    };
    if !binary_exists(&wrapped.program) {
        return;
    }

    let background_tasks = Arc::new(
        BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
            .expect("background task store"),
    );
    let tool = BashTool {
        sandbox: Arc::new(sandbox),
        background_tasks: background_tasks.clone(),
        base_env: Arc::new(HashMap::new()),
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
    };
    let status_tool = BackgroundTaskStatusTool {
        background_tasks: background_tasks.clone(),
    };

    let started = tool
        .call(json!({
            "program": "sh",
            "args": ["-c", "printf 'background-out\\n'"],
            "run_in_background": true,
        }))
        .await
        .expect("background start");
    let task_id = started
        .get("background_task_id")
        .and_then(Value::as_str)
        .expect("task id");
    assert_eq!(started.get("exit_code"), Some(&Value::Null));
    assert_eq!(
        started.get("status"),
        Some(&json!(BackgroundTaskStatus::Running))
    );
    assert_eq!(
        started.get("network_access").and_then(Value::as_bool),
        Some(wrapped.network_access)
    );

    let mut last = Value::Null;
    for _ in 0..50 {
        last = status_tool
            .call(json!({ "task_id": task_id, "tail_bytes": 4096 }))
            .await
            .expect("background status");
        if last.get("status") != Some(&json!(BackgroundTaskStatus::Running)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert_ne!(
        last.get("status"),
        Some(&json!(BackgroundTaskStatus::Running))
    );
    assert!(last.get("output_path").and_then(Value::as_str).is_some());
    assert_eq!(
        last.get("network_access").and_then(Value::as_bool),
        Some(wrapped.network_access)
    );
}

#[tokio::test]
async fn background_tasks_can_be_listed_and_stopped_without_count_limit() {
    let temp = tempdir().expect("tempdir");
    let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
    let Ok(wrapped) = sandbox.wrap_exec_command(
        "sh",
        &["-c".to_string(), "sleep 30".to_string()],
        temp.path().to_string_lossy().as_ref(),
        false,
    ) else {
        return;
    };
    if !binary_exists(&wrapped.program) {
        return;
    }

    let background_tasks = Arc::new(
        BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
            .expect("background task store"),
    );
    let tool = BashTool {
        sandbox: Arc::new(sandbox),
        background_tasks: background_tasks.clone(),
        base_env: Arc::new(HashMap::new()),
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
    };
    let list_tool = BackgroundTaskListTool {
        background_tasks: background_tasks.clone(),
    };
    let stop_tool = BackgroundTaskStopTool {
        background_tasks: background_tasks.clone(),
    };

    let started = tool
        .call(json!({
            "program": "sh",
            "args": ["-c", "sleep 30"],
            "run_in_background": true,
        }))
        .await
        .expect("background start");
    let task_id = started
        .get("background_task_id")
        .and_then(Value::as_str)
        .expect("task id")
        .to_string();

    let listed = list_tool.call(json!({})).await.expect("list tasks");
    assert_eq!(
        listed.get("tasks").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );
    assert_eq!(
        listed
            .pointer("/tasks/0/network_access")
            .and_then(Value::as_bool),
        Some(wrapped.network_access)
    );

    let stopped = stop_tool
        .call(json!({ "task_id": task_id }))
        .await
        .expect("stop task");
    assert_eq!(
        stopped.pointer("/stopped/0/status"),
        Some(&json!(BackgroundTaskStatus::Killed))
    );
    assert_eq!(
        stopped
            .pointer("/stopped/0/network_access")
            .and_then(Value::as_bool),
        Some(wrapped.network_access)
    );
}

fn binary_exists(program: &str) -> bool {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return program_path.exists();
    }

    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(program).exists()))
        .unwrap_or(false)
}
