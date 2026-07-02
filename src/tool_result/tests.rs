#[cfg(test)]
mod projection_cases {
    use serde_json::json;

    use crate::agent::Message;
    use crate::tool_result::{
        TOOL_RESULT_BATCH_BUDGET, ToolResultProjectionPolicy, ToolResultStore, compact_read_file,
        compact_subagent_result, compact_web_search, default_tool_result_store_dir,
        enforce_tool_result_batch_budget, project_tool_results_for_context,
        repair_tool_result_history, tool_result_content_candidates,
    };

    #[test]
    fn repairs_missing_tool_results() {
        let history = vec![
            Message {
                role: "assistant".into(),
                content: json!([{ "type": "tool_use", "id": "call-1", "name": "list_files", "input": {} }]),
            },
            Message {
                role: "assistant".into(),
                content: json!([{ "type": "text", "text": "follow-up" }]),
            },
        ];
        let repaired = repair_tool_result_history(&history);
        assert_eq!(repaired.len(), 3);
        assert_eq!(repaired[1].role, "user");
        assert!(repaired[1].content.to_string().contains("call-1"));
    }

    #[test]
    fn compacts_large_read_file_results() {
        let summary = compact_read_file(
            &json!({ "path": "src/main.rs" }),
            &json!({ "content": "a".repeat(10_000) }),
        );
        assert!(summary.contains("Read file src/main.rs"));
        assert!(summary.contains("truncated"));
    }

    #[test]
    fn read_file_summary_distinguishes_line_truncation_from_more_lines() {
        let summary = compact_read_file(
            &json!({ "path": "src/generated.json" }),
            &json!({
                "content": "x".repeat(4_020),
                "total_lines": 1,
                "total_lines_exact": true,
                "start_line": 1,
                "end_line": 1,
                "truncated": true,
                "line_truncated": true,
                "next_offset": null,
            }),
        );

        assert!(summary.contains("Read file src/generated.json"));
        assert!(summary.contains("Truncated line(s)."));
        assert!(!summary.contains("continue with offset"));
    }

    #[test]
    fn stores_oversized_results_on_disk() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = ToolResultStore::new(tempdir.path()).expect("store");
        let output = store
            .compact_result(
                "web_fetch",
                "tool-1",
                &json!({ "url": "https://example.com" }),
                &json!({ "content": "x".repeat(20_000) }),
            )
            .expect("compact result");
        assert!(output.contains("full result:"));
        assert!(output.contains("Fetched https://example.com"));
        assert!(tempdir.path().join("tool-1.json").exists());
        assert!(
            default_tool_result_store_dir()
                .expect("default tool result dir")
                .ends_with(std::path::Path::new("tool-results"))
        );
    }

    #[test]
    fn compacts_bash_results_with_exit_code_duration_and_aggregated_output() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = ToolResultStore::new(tempdir.path()).expect("store");
        let output = store
            .compact_result(
                "bash",
                "tool-bash",
                &json!({ "program": "cargo", "args": ["check"] }),
                &json!({
                    "stdout": "stdout-only\n",
                    "stderr": "stderr-only\n",
                    "aggregated_output": "checking\n[stderr] warning\n",
                    "exit_code": 101,
                    "duration_ms": 1234,
                    "live_streamed": true,
                    "sandboxed": true,
                    "sandbox_backend": "macos-seatbelt"
                }),
            )
            .expect("compact bash result");

        assert!(output.contains("failed with exit code 101"));
        assert!(output.contains("Duration: 1234 ms"));
        assert!(output.contains("Output:\nchecking\n[stderr] warning"));
        assert!(!output.contains("stdout-only"));
        assert!(!output.contains("stderr-only"));
    }

    #[test]
    fn compacts_bash_prefers_independent_model_preview_output() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = ToolResultStore::new(tempdir.path()).expect("store");
        let output = store
            .compact_result(
                "bash",
                "tool-bash",
                &json!({ "program": "cargo", "args": ["test"] }),
                &json!({
                    "stdout": "stdout-only\n",
                    "stderr": "stderr-only\n",
                    "aggregated_output": "full aggregated output\n",
                    "model_preview_output": "model preview output\n",
                    "exit_code": 1,
                    "duration_ms": 10
                }),
            )
            .expect("compact bash result");

        assert!(output.contains("model preview output"));
        assert!(!output.contains("full aggregated output"));
        assert!(!output.contains("stdout-only"));
        assert!(!output.contains("stderr-only"));
    }

    #[test]
    fn compacts_bash_long_aggregated_output_with_head_and_tail() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = ToolResultStore::new(tempdir.path()).expect("store");
        let output = store
            .compact_result(
                "bash",
                "tool-bash",
                &json!({ "program": "cargo", "args": ["test"] }),
                &json!({
                    "aggregated_output": format!("head\n{}tail-error\n", "middle\n".repeat(2_000)),
                    "exit_code": 1,
                    "duration_ms": 10
                }),
            )
            .expect("compact bash result");

        assert!(output.contains("head"));
        assert!(output.contains("tail-error"));
        assert!(output.contains("chars truncated from middle"));
    }

    #[test]
    fn compacts_bash_unknown_exit_status_as_error_preview() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = ToolResultStore::new(tempdir.path()).expect("store");
        let output = store
            .compact_result(
                "bash",
                "tool-bash",
                &json!({ "program": "cargo", "args": ["test"] }),
                &json!({
                    "aggregated_output": format!("head\n{}tail-error\n", "middle\n".repeat(2_000)),
                    "duration_ms": 10
                }),
            )
            .expect("compact bash result");

        assert!(output.contains("head"));
        assert!(output.contains("tail-error"));
        assert!(output.contains("chars truncated from middle"));
    }

    #[test]
    fn batch_budget_shortens_largest_tool_results() {
        let large = "large-start\n".to_string() + &"middle\n".repeat(4_000) + "large-tail\n";
        let small = "small-result\n".to_string();
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: json!([{
                    "type": "tool_result",
                    "tool_use_id": "large",
                    "content": large,
                }]),
            },
            Message {
                role: "user".to_string(),
                content: json!([{
                    "type": "tool_result",
                    "tool_use_id": "small",
                    "content": small,
                }]),
            },
        ];

        let budgeted = enforce_tool_result_batch_budget(messages);
        let large_content = budgeted[0].content[0]["content"]
            .as_str()
            .expect("large content");
        let small_content = budgeted[1].content[0]["content"]
            .as_str()
            .expect("small content");

        assert!(large_content.contains("large-start"));
        assert!(large_content.contains("large-tail"));
        assert!(large_content.contains("tool_result shortened"));
        assert_eq!(small_content, "small-result\n");
    }

    #[test]
    fn batch_budget_enforces_final_total_for_many_large_results() {
        let large = "large-start\n".to_string() + &"middle\n".repeat(4_000) + "large-tail\n";
        let messages = (0..20)
            .map(|idx| Message {
                role: "user".to_string(),
                content: json!([{
                    "type": "tool_result",
                    "tool_use_id": format!("large-{idx}"),
                    "content": large,
                }]),
            })
            .collect::<Vec<_>>();

        let budgeted = enforce_tool_result_batch_budget(messages);
        let total_chars = tool_result_content_candidates(&budgeted)
            .iter()
            .map(|candidate| candidate.chars)
            .sum::<usize>();

        assert!(total_chars <= TOOL_RESULT_BATCH_BUDGET);
        assert!(
            budgeted
                .iter()
                .filter_map(|message| message.content[0]["content"].as_str())
                .any(|content| content.contains("tool_result shortened"))
        );
    }

    #[test]
    fn projects_old_compactable_tool_results_without_changing_source_messages() {
        let messages = (0..4)
            .flat_map(|idx| {
                [
                    Message {
                        role: "assistant".to_string(),
                        content: json!([{
                            "type": "tool_use",
                            "id": format!("tool-{idx}"),
                            "name": "read_file",
                            "input": {}
                        }]),
                    },
                    Message {
                        role: "user".to_string(),
                        content: json!([{
                            "type": "tool_result",
                            "tool_use_id": format!("tool-{idx}"),
                            "content": format!("result-{idx}-{}", "x".repeat(128))
                        }]),
                    },
                ]
            })
            .collect::<Vec<_>>();

        let (projected, report) = project_tool_results_for_context(
            &messages,
            &ToolResultProjectionPolicy {
                enabled: true,
                budget_chars: 180,
                keep_recent: 1,
                cache_edit_eligible: false,
            },
        );
        let projected_text = serde_json::to_string(&projected).expect("projected json");
        let source_text = serde_json::to_string(&messages).expect("source json");

        assert!(report.cleared_results > 0);
        assert!(projected_text.contains("Old tool result content cleared"));
        assert!(!projected_text.contains("result-0-"));
        assert!(projected_text.contains("result-3-"));
        assert!(source_text.contains("result-0-"));
    }

    #[test]
    fn projection_ignores_non_compactable_tool_results() {
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: json!([{
                    "type": "tool_use",
                    "id": "tool-1",
                    "name": "remember_project_memory",
                    "input": {}
                }]),
            },
            Message {
                role: "user".to_string(),
                content: json!([{
                    "type": "tool_result",
                    "tool_use_id": "tool-1",
                    "content": format!("memory-result-{}", "x".repeat(1_000))
                }]),
            },
        ];

        let (projected, report) = project_tool_results_for_context(
            &messages,
            &ToolResultProjectionPolicy {
                enabled: true,
                budget_chars: 1,
                keep_recent: 1,
                cache_edit_eligible: false,
            },
        );

        assert_eq!(projected, messages);
        assert_eq!(report.cleared_results, 0);
    }

    #[test]
    fn projection_reports_provider_cache_edit_gate_without_applying_cache_edits() {
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: json!([{
                    "type": "tool_use",
                    "id": "tool-1",
                    "name": "read_file",
                    "input": {}
                }]),
            },
            Message {
                role: "user".to_string(),
                content: json!([{
                    "type": "tool_result",
                    "tool_use_id": "tool-1",
                    "content": "small result"
                }]),
            },
        ];

        let (_projected, report) = project_tool_results_for_context(
            &messages,
            &ToolResultProjectionPolicy::default().for_provider_cache_edit(true),
        );

        assert!(report.cache_edit_eligible);
        assert!(!report.cache_edit_applied);
    }

    #[test]
    fn compacts_bash_fallback_separates_stdout_and_stderr() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = ToolResultStore::new(tempdir.path()).expect("store");
        let output = store
            .compact_result(
                "bash",
                "tool-bash",
                &json!({ "program": "sh" }),
                &json!({
                    "stdout": "stdout-without-newline",
                    "stderr": "stderr-line\n",
                    "exit_code": 1,
                    "duration_ms": 10,
                }),
            )
            .expect("compact bash result");

        assert!(output.contains("stdout-without-newline\n[stderr] stderr-line"));
    }

    #[test]
    fn compacts_bash_fallback_prefixes_each_stderr_line() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = ToolResultStore::new(tempdir.path()).expect("store");
        let output = store
            .compact_result(
                "bash",
                "tool-bash",
                &json!({ "program": "sh" }),
                &json!({
                    "stdout": "",
                    "stderr": "first\nsecond\n",
                    "exit_code": 1,
                    "duration_ms": 10,
                }),
            )
            .expect("compact bash result");

        assert!(output.contains("[stderr] first\n[stderr] second\n"));
    }

    #[test]
    fn compacts_background_bash_with_task_id() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = ToolResultStore::new(tempdir.path()).expect("store");
        let output = store
            .compact_result(
                "bash",
                "tool-bash",
                &json!({ "program": "sh", "run_in_background": true }),
                &json!({
                    "background_task_id": "bash-123",
                    "status": "running",
                    "output_path": "/tmp/rara/bash-123.log",
                    "exit_code": null,
                    "stdout": "",
                    "stderr": "",
                }),
            )
            .expect("compact background bash result");

        assert!(output.contains("bash started in background."));
        assert!(output.contains("Task id: bash-123"));
        assert!(output.contains("Status: running"));
        assert!(output.contains("background_task_status"));
    }

    #[test]
    fn replace_results_include_renderable_diff_preview() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = ToolResultStore::new(tempdir.path()).expect("store");
        let output = store
            .compact_result(
                "replace",
                "tool-1",
                &json!({
                    "path": "src/main.rs",
                    "old_string": "let old = true;",
                    "new_string": "let new = true;"
                }),
                &json!({
                    "status": "ok",
                    "path": "src/main.rs",
                    "replacements": 1,
                    "line_delta": 0
                }),
            )
            .expect("compact replace");

        assert!(output.contains("diff:"));
        assert!(output.contains("*** Update File: src/main.rs"));
        assert!(output.contains("-let old = true;"));
        assert!(output.contains("+let new = true;"));
    }

    #[test]
    fn compacts_web_search_with_preview() {
        let output = compact_web_search(
            &json!({ "query": "rara exa mcp" }),
            &json!({
                "query": "rara exa mcp",
                "provider": "exa_mcp",
                "content": "Result one\nResult two"
            }),
        );

        assert!(output.contains("Searched web for \"rara exa mcp\""));
        assert!(output.contains("Results:"));
        assert!(output.contains("Result one"));
    }

    #[test]
    fn compacts_subagent_results_without_full_payload() {
        let compacted = compact_subagent_result(
            "spawn_agent",
            &json!({
                "agent_id": "fix-assembler-123",
                "session_id": "child-session-123",
                "name": "fix-assembler",
                "status": "done",
                "summary": "Removed the orphaned test block and kept one cfg(test) module.",
                "request_user_input": {
                    "question": "Proceed?",
                    "options": [
                        ["Yes", "Apply the cleanup."],
                        { "label": "No", "description": "Leave the file unchanged." }
                    ],
                    "note": "The line range was verified."
                }
            }),
        );

        assert!(compacted.starts_with("spawn_agent fix-assembler: Removed"));
        assert!(compacted.contains("agent_id: fix-assembler-123"));
        assert!(compacted.contains("session_id: child-session-123"));
        assert!(compacted.contains("request_user_input: Proceed?"));
        assert!(compacted.contains("option: Yes | Apply the cleanup."));
        assert!(compacted.contains("option: No | Leave the file unchanged."));
        assert!(compacted.contains("note: The line range was verified."));
        assert!(!compacted.contains("\"summary\""));
        assert!(!compacted.contains("\"request_user_input\""));
    }
}
