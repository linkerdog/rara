use tempfile::tempdir;

use super::specs::normalize_command_token;
use super::status::truncate_preview;
use super::{
    COMMAND_SPECS, help_text, matching_commands, model_help_text, palette_commands,
    parse_local_command, recommended_commands, status_context_text, status_prompt_sources_text,
    status_resources_text, status_runtime_text,
};
use crate::config::{ConfigManager, OpenAiEndpointKind};
use crate::context::{PromptSourceContextEntry, TodoContextView};
use crate::todo::TodoSummary;
use crate::tui::state::{LocalCommandKind, RuntimeSnapshot, TuiApp};

#[test]
fn parses_model_command_argument() {
    let command = parse_local_command("/model anything").expect("command should parse");
    assert!(matches!(command.kind, LocalCommandKind::Model));
    assert_eq!(command.arg.as_deref(), Some("anything"));
}

#[test]
fn model_help_text_labels_deepseek_as_openai_compatible_endpoint() {
    let dir = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_model(Some("deepseek-chat".to_string()));
    app.set_deepseek_model_options(vec!["deepseek-chat".to_string()]);

    let rendered = model_help_text(&app);

    assert!(rendered.contains("* 1. deepseek-chat (openai-compatible:deepseek/deepseek-chat)"));
    assert!(!rendered.contains("(deepseek/deepseek-chat)"));
}

#[test]
fn parses_context_command() {
    let command = parse_local_command("/context").expect("command should parse");
    assert!(matches!(command.kind, LocalCommandKind::Context));
    assert!(command.arg.is_none());
}

#[test]
fn parses_alias_commands() {
    let runtime = parse_local_command("/runtime").expect("runtime should parse");
    assert!(matches!(runtime.kind, LocalCommandKind::Status));

    let memory = parse_local_command("/memory").expect("memory should parse");
    assert!(matches!(memory.kind, LocalCommandKind::Context));

    let threads = parse_local_command("/threads").expect("threads should parse");
    assert!(matches!(threads.kind, LocalCommandKind::Resume));

    let auth = parse_local_command("/auth").expect("auth should parse");
    assert!(matches!(auth.kind, LocalCommandKind::Login));

    let models = parse_local_command("/models").expect("models should parse");
    assert!(matches!(models.kind, LocalCommandKind::Model));
}

#[test]
fn returns_none_for_unknown_command() {
    assert!(parse_local_command("/unknown").is_none());
}

#[test]
fn parses_quit_aliases() {
    let quit = parse_local_command("/quit").expect("quit should parse");
    assert!(matches!(quit.kind, LocalCommandKind::Quit));

    let exit = parse_local_command("/exit").expect("exit should parse");
    assert!(matches!(exit.kind, LocalCommandKind::Quit));
}

#[test]
fn parses_base_url_command() {
    let command = parse_local_command("/base-url").expect("command should parse");
    assert!(matches!(command.kind, LocalCommandKind::BaseUrl));
    assert_eq!(command.arg.as_deref(), None);
}

#[test]
fn parses_resume_command() {
    let command = parse_local_command("/resume").expect("command should parse");
    assert!(matches!(command.kind, LocalCommandKind::Resume));
}

#[test]
fn parses_plan_command() {
    let plan = parse_local_command("/plan").expect("plan should parse");
    assert!(matches!(plan.kind, LocalCommandKind::Plan));
}

#[test]
fn parses_approval_command() {
    let approval = parse_local_command("/approval").expect("approval should parse");
    assert!(matches!(approval.kind, LocalCommandKind::Approval));
}

#[test]
fn parses_permissions_command() {
    let cmd = parse_local_command("/permissions").expect("/permissions should parse");
    assert!(matches!(cmd.kind, LocalCommandKind::Permissions));
    assert!(cmd.arg.is_none());

    let alias = parse_local_command("/permission").expect("/permission should parse");
    assert!(matches!(alias.kind, LocalCommandKind::Permissions));
    assert!(alias.arg.is_none());
}

#[test]
fn parses_mcp_command() {
    let command = parse_local_command("/mcp").expect("/mcp should parse");
    assert!(matches!(command.kind, LocalCommandKind::Mcp));
    assert!(command.arg.is_none());
}

#[test]
fn parses_compact_command() {
    let command = parse_local_command("/compact").expect("compact should parse");
    assert!(matches!(command.kind, LocalCommandKind::Compact));
    assert_eq!(command.arg.as_deref(), None);
}

#[test]
fn parses_login_and_logout_commands() {
    let login = parse_local_command("/login").expect("login should parse");
    assert!(matches!(login.kind, LocalCommandKind::Login));
    assert_eq!(login.arg.as_deref(), None);

    let logout = parse_local_command("/logout").expect("logout should parse");
    assert!(matches!(logout.kind, LocalCommandKind::Logout));
    assert_eq!(logout.arg.as_deref(), None);
}

#[test]
fn matches_commands_by_prefix() {
    let names = matching_commands("st")
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    assert_eq!(names.first().copied(), Some("status"));
    assert!(!names.contains(&"setup"));
}

#[test]
fn normalizes_model_labels_for_command_matching() {
    assert_eq!(normalize_command_token("Gemma 4 E4B"), "gemma4e4b");
    assert_eq!(normalize_command_token("Qwn3 8B"), "qwn38b");
}

#[test]
fn exact_and_prefix_matches_rank_ahead_of_fuzzy_matches() {
    let names = matching_commands("model")
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    assert_eq!(names.first().copied(), Some("model"));
}

#[test]
fn palette_commands_are_sorted_alphabetically_for_empty_query() {
    let dir = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");

    let names = palette_commands(&app, "")
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn palette_commands_show_full_command_list_for_empty_query() {
    let dir = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");

    let names = palette_commands(&app, "")
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    assert_eq!(names.len(), COMMAND_SPECS.len());
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn recommended_commands_restore_context_model_resume_and_status() {
    let dir = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");

    let names = recommended_commands(&app)
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["context", "help", "model", "resume", "status"]);
}

#[test]
fn help_text_lists_built_in_commands_alphabetically() {
    let rendered = help_text();
    let approval_idx = rendered.find("/approval").expect("approval");
    let base_url_idx = rendered.find("/base-url").expect("base-url");
    assert!(approval_idx < base_url_idx);
}

#[test]
fn truncate_preview_handles_unicode_and_small_limits() {
    assert_eq!(truncate_preview("alpha beta", 0), "");
    assert_eq!(truncate_preview("alpha beta", 1), "…");
    assert_eq!(truncate_preview("你好 世界", 3), "你好…");
    assert_eq!(truncate_preview("alpha   beta", 20), "alpha beta");
}

#[test]
fn status_runtime_text_reports_model_and_reasoning_sources() {
    let dir = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");

    app.config.set_provider("openai-compatible");
    app.config
        .set_base_url(Some("http://proxy.local/v1".to_string()));
    app.config.set_model(Some("custom-model".to_string()));
    app.config
        .set_reasoning_summary(Some("detailed".to_string()));

    let rendered = status_runtime_text(&app);
    assert!(rendered.contains("endpoint_profile=Custom endpoint"));
    assert!(rendered.contains("endpoint_kind=custom"));
    assert!(rendered.contains("model=custom-model"));
    assert!(rendered.contains("model_source=provider_state"));
    assert!(rendered.contains("base_url_source=provider_state"));
    assert!(rendered.contains("api_key_source=unset"));
    assert!(rendered.contains("reasoning_summary=detailed"));
    assert!(rendered.contains("reasoning_summary_source=provider_state"));
    assert!(rendered.contains("reasoning_effort_source=unset"));
    assert!(rendered.contains("revision_source=unset"));
}

#[test]
fn status_runtime_text_reports_active_openai_endpoint_profile() {
    let dir = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");

    app.config.select_openai_profile(
        "openrouter-main",
        "OpenRouter main",
        rara_config::OpenAiEndpointKind::Openrouter,
    );
    app.config
        .set_model(Some("anthropic/claude-sonnet-4".to_string()));

    let rendered = status_runtime_text(&app);
    assert!(rendered.contains("provider=openai-compatible"));
    assert!(rendered.contains("endpoint_profile=OpenRouter main"));
    assert!(rendered.contains("endpoint_kind=openrouter"));
    assert!(rendered.contains("model=anthropic/claude-sonnet-4"));
}

#[test]
fn status_runtime_text_reports_codex_auth_surface() {
    let dir = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");

    app.config.set_provider("codex");
    app.config.set_base_url(Some(
        rara_config::DEFAULT_CODEX_CHATGPT_BASE_URL.to_string(),
    ));
    app.config
        .set_model(Some(rara_config::DEFAULT_CODEX_MODEL.to_string()));
    app.codex_auth_mode = Some(crate::oauth::SavedCodexAuthMode::Chatgpt);

    let rendered = status_runtime_text(&app);
    assert!(rendered.contains("codex_auth_mode=chatgpt"));
    assert!(rendered.contains("codex_endpoint_kind=chatgpt_codex"));
}

#[test]
fn status_prompt_sources_text_includes_structured_entries() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.snapshot.prompt_base_kind = "default".to_string();
    app.snapshot.prompt_section_keys =
        vec!["instructions".to_string(), "runtime_context".to_string()];
    app.snapshot.prompt_source_entries = vec![PromptSourceContextEntry {
        order: 1,
        kind: "project_instruction".to_string(),
        label: "Project Instruction (AGENTS.md)".to_string(),
        display_path: "AGENTS.md".to_string(),
        status_line: "project instruction: AGENTS.md".to_string(),
        inclusion_reason: "included because workspace instruction discovery found this file in the active workspace ancestry".to_string(),
    }];

    let rendered = status_prompt_sources_text(&app);
    assert!(
        rendered.contains("1. Project Instruction (AGENTS.md) [project_instruction] AGENTS.md")
    );
    assert!(
        rendered.contains("why: included because workspace instruction discovery found this file")
    );
}

#[test]
fn status_runtime_text_reports_effective_provider_surface_sources() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.config.set_provider("codex");

    let rendered = status_runtime_text(&app);
    assert!(rendered.contains("model_source="));
    assert!(rendered.contains("base_url_source="));
    assert!(rendered.contains("revision_source="));
    assert!(rendered.contains("reasoning_summary_source=legacy_global"));
}

#[test]
fn status_context_text_includes_prompt_sources_and_plan_state() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.snapshot = RuntimeSnapshot {
        cwd: "/workspace/rara".into(),
        branch: "main".into(),
        session_id: "session-123".into(),
        estimated_history_tokens: 12_345,
        context_window_tokens: Some(200_000),
        compact_threshold_tokens: 180_000,
        reserved_output_tokens: 8_192,
        stable_instructions_budget: 1_200,
        workspace_prompt_budget: 320,
        active_turn_budget: 280,
        compacted_history_budget: 140,
        retrieved_memory_budget: 96,
        remaining_input_budget: Some(189_772),
        compaction_count: 1,
        last_compaction_before_tokens: Some(12_000),
        last_compaction_after_tokens: Some(4_500),
        last_compaction_boundary_version: Some(1),
        last_compaction_boundary_before_tokens: Some(12_000),
        last_compaction_boundary_recent_file_count: Some(2),
        compaction_source_entries: vec![crate::context::CompactionSourceContextEntry {
            order: 1,
            kind: "compacted_summary".into(),
            label: "Compacted Thread Summary".into(),
            source_descriptor: "history.compaction.summary".into(),
            detail: "User Intent".into(),
            inclusion_reason:
                "included because older thread history was compacted into a structured summary instead of being replayed verbatim".into(),
        }],
        retrieval_source_entries: vec![
            crate::context::RetrievalSourceContextEntry {
                order: 1,
                kind: "workspace_memory".into(),
                label: "Workspace Memory".into(),
                status: "active".into(),
                detail: "/workspace/rara/.rara/memory.md".into(),
                inclusion_reason:
                    "included now because the local workspace memory file was discovered as an explicit prompt source".into(),
            },
        ],
        retrieval_orchestration: crate::context::RetrievalOrchestrationView {
            request_id: "session-123".into(),
            query: "continue context work".into(),
            providers: vec![crate::context::RetrievalProviderStatus {
                order: 1,
                kind: "workspace_memory".into(),
                label: "Workspace Memory".into(),
                status: "active".into(),
                detail: "/workspace/rara/.rara/memory.md".into(),
                inclusion_reason:
                    "included now because the local workspace memory file was discovered as an explicit prompt source".into(),
            }],
            selected: vec![crate::context::RetrievalCandidateContextEntry {
                order: 1,
                kind: "retrieved_workspace_memory".into(),
                label: "Retrieved Experience".into(),
                detail: "content: prior implementation detail".into(),
                status: "selected".into(),
                source_kind: "memory_record".into(),
                budget_impact_tokens: Some(32),
                reason:
                    "selected because the retrieval tool returned relevant durable memory candidates for the current task".into(),
            }],
            available: vec![crate::context::RetrievalCandidateContextEntry {
                order: 1,
                kind: "thread_history".into(),
                label: "Thread History".into(),
                detail: "session=session-123 messages=10".into(),
                status: "available".into(),
                source_kind: "thread_history".into(),
                budget_impact_tokens: None,
                reason:
                    "available for recall, but not selected into the current assembled context".into(),
            }],
            dropped: Vec::new(),
            candidates: vec![
                crate::context::RetrievalCandidateContextEntry {
                    order: 1,
                    kind: "retrieved_workspace_memory".into(),
                    label: "Retrieved Experience".into(),
                    detail: "content: prior implementation detail".into(),
                    status: "selected".into(),
                    source_kind: "memory_record".into(),
                    budget_impact_tokens: Some(32),
                    reason:
                        "selected because the retrieval tool returned relevant durable memory candidates for the current task".into(),
                },
                crate::context::RetrievalCandidateContextEntry {
                    order: 2,
                    kind: "thread_history".into(),
                    label: "Thread History".into(),
                    detail: "session=session-123 messages=10".into(),
                    status: "available".into(),
                    source_kind: "thread_history".into(),
                    budget_impact_tokens: None,
                    reason:
                        "available for recall, but not selected into the current assembled context".into(),
                },
            ],
            budget: crate::context::RetrievalBudgetContextView {
                selection_budget_tokens: Some(189_772),
                selected_tokens: 32,
                available_tokens: 0,
                dropped_tokens: 0,
            },
        },
        assembly_entries: vec![
            crate::context::ContextAssemblyEntry {
                cache_status: None,
                order: 1,
                layer: "stable_instructions".into(),
                kind: "project_instruction".into(),
                label: "Project Instruction (AGENTS.md)".into(),
                source_path: Some("AGENTS.md".into()),
                injected: true,
                inclusion_reason:
                    "included because workspace instruction discovery found this file".into(),
                budget_impact_tokens: Some(240),
                dropped_reason: None,
            },
            crate::context::ContextAssemblyEntry {
                cache_status: Some(crate::context::CacheStatus::NoCache),
                order: 2,
                layer: "active_memory_inputs".into(),
                kind: "workspace_memory".into(),
                label: "Workspace Memory".into(),
                source_path: Some("/workspace/rara/.rara/memory.md".into()),
                injected: true,
                inclusion_reason:
                    "selected because the current effective prompt includes the workspace memory file as an active input".into(),
                budget_impact_tokens: Some(64),
                dropped_reason: None,
            },
            crate::context::ContextAssemblyEntry {
                cache_status: Some(crate::context::CacheStatus::NoCache),
                order: 3,
                layer: "active_memory_inputs".into(),
                kind: "retrieved_workspace_memory".into(),
                label: "Retrieved Experience".into(),
                source_path: None,
                injected: true,
                inclusion_reason:
                    "selected because the retrieval tool returned relevant durable memory candidates for the current task".into(),
                budget_impact_tokens: Some(32),
                dropped_reason: None,
            },
            crate::context::ContextAssemblyEntry {
                cache_status: None,
                order: 4,
                layer: "compacted_history".into(),
                kind: "compacted_summary".into(),
                label: "Compacted Thread Summary".into(),
                source_path: Some("history.compaction.summary".into()),
                injected: true,
                inclusion_reason:
                    "included because older thread history was compacted into a structured summary instead of being replayed verbatim".into(),
                budget_impact_tokens: Some(48),
                dropped_reason: None,
            },
            crate::context::ContextAssemblyEntry {
                cache_status: None,
                order: 5,
                layer: "active_turn_state".into(),
                kind: "plan_steps".into(),
                label: "Plan Steps".into(),
                source_path: None,
                injected: true,
                inclusion_reason:
                    "included because structured plan steps are part of the current active thread state".into(),
                budget_impact_tokens: Some(80),
                dropped_reason: None,
            },
            crate::context::ContextAssemblyEntry {
                cache_status: None,
                order: 6,
                layer: "retrieval_ready".into(),
                kind: "thread_history".into(),
                label: "Thread History".into(),
                source_path: None,
                injected: false,
                inclusion_reason:
                    "available as the session-local history source for restore and future recall surfaces".into(),
                budget_impact_tokens: None,
                dropped_reason:
                    Some("available for recall, but not selected into the current assembled context".into()),
            },
        ],
        plan_steps: vec![("pending".into(), "Implement /context".into())],
        todo: TodoContextView {
            summary: TodoSummary {
                total: 3,
                pending: 1,
                in_progress: 1,
                completed: 1,
                cancelled: 0,
                active_item: Some("Wire todo state into /context".into()),
            },
            updated_at: Some(1_777_584_000),
            items: vec![
                (
                    "todo-1".into(),
                    "completed".into(),
                    "Implement todo_write runtime".into(),
                ),
                (
                    "todo-2".into(),
                    "in_progress".into(),
                    "Wire todo state into /context".into(),
                ),
                ("todo-3".into(), "pending".into(), "Add status coverage".into()),
            ],
        },
        todo_artifact_path: Some("/workspace/rara/.rara/sessions/session-123/todo.json".into()),
        ..RuntimeSnapshot::default()
    };

    let rendered = status_context_text(&app);
    assert!(rendered.contains("Context Usage"));
    assert!(rendered.contains("model:"));
    assert!(rendered.contains("used: 10228 tokens (5.1%) / 200000 tokens"));
    assert!(rendered.contains("System prompt: 1520 tokens (0.8%)"));
    assert!(rendered.contains("Free space: 189772 tokens (94.9%)"));
    assert!(rendered.contains("Autocompact buffer: 20000 tokens (10.0%)"));
    assert!(rendered.contains("Session"));
    assert!(rendered.contains("Budget"));
    assert!(rendered.contains("stable: 1200 tokens"));
    assert!(rendered.contains("Stable Instructions"));
    assert!(rendered.contains("Workspace Prompt Sources"));
    assert!(rendered.contains("Active Memory Inputs"));
    assert!(rendered.contains("Retrieval Orchestration"));
    assert!(rendered.contains("query: continue context work"));
    assert!(rendered.contains("[workspace_memory] Workspace Memory status=active"));
    assert!(rendered.contains("[memory_record/retrieved_workspace_memory] Retrieved Experience"));
    assert!(rendered.contains("Compacted History"));
    assert!(rendered.contains("path: history.compaction.summary"));
    assert!(rendered.contains("Active Turn State"));
    assert!(rendered.contains("Retrieval-ready"));
    assert!(rendered.contains("Plan"));
    assert!(rendered.contains("[pending] Implement /context"));
    assert!(rendered.contains("Todo"));
    assert!(rendered.contains("artifact: /workspace/rara/.rara/sessions/session-123/todo.json"));
    assert!(rendered.contains("updated_at: 2026-04-30 21:20:00 UTC"));
    assert!(rendered.contains("total: 3  pending: 1  in_progress: 1  completed: 1"));
    assert!(!rendered.contains("active: Wire todo state into /context"));
    assert!(rendered.contains("[in_progress] Wire todo state into /context (todo-2, active)"));
    assert!(rendered.contains("Pending"));
}

#[test]
fn status_runtime_text_reports_todo_summary() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.snapshot = RuntimeSnapshot {
        todo: TodoContextView {
            summary: TodoSummary {
                total: 2,
                pending: 1,
                in_progress: 1,
                completed: 0,
                cancelled: 0,
                active_item: Some("Run focused tests".into()),
            },
            updated_at: Some(1_777_584_000),
            items: vec![
                (
                    "todo-1".into(),
                    "in_progress".into(),
                    "Run focused tests".into(),
                ),
                ("todo-2".into(), "pending".into(), "Push PR update".into()),
            ],
        },
        ..RuntimeSnapshot::default()
    };

    let rendered = status_runtime_text(&app);
    assert!(rendered.contains(
        "todo=2 total, 1 pending, 1 in_progress, 0 completed, 0 cancelled, active=Run focused tests"
    ));
}

#[test]
fn status_resources_text_includes_token_and_cache_summary() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.snapshot.total_input_tokens = 1_000;
    app.snapshot.total_output_tokens = 500;
    app.snapshot.total_cache_hit_tokens = 300;
    app.snapshot.total_cache_miss_tokens = 100;
    app.snapshot.estimated_history_tokens = 12_000;
    app.snapshot.context_window_tokens = Some(200_000);
    app.snapshot.compact_threshold_tokens = 180_000;
    app.snapshot.reserved_output_tokens = 8_192;
    app.snapshot.compaction_count = 2;
    app.snapshot.last_compaction_before_tokens = Some(10_000);
    app.snapshot.last_compaction_after_tokens = Some(3_000);
    app.snapshot.last_compaction_recent_files =
        vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
    app.snapshot.last_compaction_boundary_recent_file_count = Some(3);
    app.snapshot.compaction_source_entries = vec![crate::context::CompactionSourceContextEntry {
        order: 1,
        kind: "compacted_summary".into(),
        label: "Compacted Thread Summary".into(),
        source_descriptor: "history.compaction.summary".into(),
        detail: "User Intent".into(),
        inclusion_reason: "compacted older history".into(),
    }];
    app.config.set_provider("local-candle");
    app.state_db_status = Some("sqlite:/tmp/rara/state.db".into());
    app.snapshot.memory_selection.selection_budget_tokens = Some(20_000);
    app.snapshot.memory_selection.selected_items =
        vec![crate::context::MemorySelectionItemContextEntry {
            order: 1,
            kind: "workspace_memory".into(),
            label: "Workspace Memory".into(),
            detail: "/workspace/.rara/memory.md".into(),
            selection_reason: "workspace memory selected".into(),
            budget_impact_tokens: Some(64),
            dropped_reason: None,
        }];

    let rendered = status_resources_text(&app);
    assert!(rendered.contains("tokens=1000 in / 500 out"));
    assert!(rendered.contains("cache_hit_tokens=300"));
    assert!(rendered.contains("cache_miss_tokens=100"));
    assert!(rendered.contains("compactions=2"));
    assert!(rendered.contains(
        "context_estimate=12000 tokens / 200000 tokens (~auto @ 180000 tokens, reserve 8192 tokens)"
    ));
    assert!(rendered.contains("last: 10000 tokens -> 3000 tokens, ratio: 30.0%"));
    assert!(rendered.contains("recent_compact_file_count=2"));
    assert!(rendered.contains("recent_compact_files=src/main.rs, src/lib.rs"));
    assert!(rendered.contains("state_db=sqlite:/tmp/rara/state.db"));
    assert!(!rendered.contains("cache=sqlite:/tmp/rara/state.db"));
}

#[test]
fn parses_skills_command() {
    let command = parse_local_command("/skills").expect("/skills should parse");
    assert!(matches!(command.kind, LocalCommandKind::Skills));
    assert!(command.arg.is_none());
}

#[test]
fn skills_command_spec_is_registered() {
    let spec = COMMAND_SPECS
        .iter()
        .find(|s| s.name == "skills")
        .expect("skills should be in COMMAND_SPECS");
    assert_eq!(spec.usage, "/skills");
    assert!(spec.summary.contains("View and toggle"));
}

#[test]
fn skills_picker_render_shows_entries() {
    use crate::tui::state::SkillPickerEntry;

    let dir = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");

    app.skill_picker_entries = vec![
        SkillPickerEntry {
            name: "reviewer".into(),
            title: "Reviewer".into(),
            scope: "cwd".into(),
            display_path: ".agents/skills/reviewer/SKILL.md".into(),
            enabled: true,
            disable_model_invocation: false,
        },
        SkillPickerEntry {
            name: "restricted".into(),
            title: "Restricted".into(),
            scope: "home".into(),
            display_path: "~/.rara/skills/restricted/SKILL.md".into(),
            enabled: false,
            disable_model_invocation: true,
        },
    ];
    app.skill_picker_idx = 1;
    app.overlay = Some(crate::tui::state::Overlay::SkillsPicker);

    // Verify entries exist and the correct one is selected
    assert_eq!(app.skill_picker_entries.len(), 2);
    assert_eq!(app.skill_picker_idx, 1);
    assert!(!app.skill_picker_entries[1].enabled);
    assert!(app.skill_picker_entries[1].disable_model_invocation);
    assert!(app.skill_picker_entries[0].enabled);
}

#[test]
fn parses_goal_command_with_objective() {
    let command = parse_local_command("/goal fix the build").expect("/goal should parse");
    assert!(matches!(command.kind, LocalCommandKind::Goal));
    assert_eq!(command.arg.as_deref(), Some("fix the build"));
}

#[test]
fn parses_goal_command_with_subcommand() {
    let command = parse_local_command("/goal pause").expect("/goal should parse");
    assert!(matches!(command.kind, LocalCommandKind::Goal));
    assert_eq!(command.arg.as_deref(), Some("pause"));
}

#[test]
fn parses_goal_bare_command_without_argument() {
    let command = parse_local_command("/goal").expect("/goal should parse");
    assert!(matches!(command.kind, LocalCommandKind::Goal));
    assert!(command.arg.is_none());
}

#[test]
fn goal_command_spec_is_registered() {
    let spec = COMMAND_SPECS
        .iter()
        .find(|s| s.name == "goal")
        .expect("goal should be in COMMAND_SPECS");
    assert_eq!(spec.usage, "/goal");
    assert_eq!(spec.summary, "Manage the active thread goal.");
    assert!(spec.detail.contains("/goal --tokens <N>"));
    assert!(spec.detail.contains("/goal <objective>"));
    assert!(spec.detail.contains("/goal pause"));
    assert!(spec.detail.contains("/goal resume"));
    assert!(spec.detail.contains("/goal clear"));
}

#[test]
fn goal_appears_in_palette_commands() {
    let temp = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build app");
    let results = palette_commands(&app, "goal");
    assert!(results.iter().any(|s| s.name == "goal"));
}
