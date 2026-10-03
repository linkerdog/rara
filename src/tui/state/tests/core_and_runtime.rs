use super::*;
use crate::tui::message_role::MessageRole;

#[test]
fn detects_slash_command_input() {
    assert!(input_requests_command_palette("/"));
    assert!(input_requests_command_palette("/help"));
    assert!(input_requests_command_palette("   /help"));
    assert!(!input_requests_command_palette(""));
    assert!(!input_requests_command_palette("help"));
    assert!(!input_requests_command_palette("   help"));
}

#[test]
fn redacts_secrets_in_state_db_status_messages() {
    let rendered = state_db_status_error(
        "write failed",
        "token=supersecretvalue Authorization: Bearer abcdefghijklmnopqrstuvwxyz",
    );
    assert!(rendered.contains("write failed:"));
    assert!(rendered.contains("[REDACTED_SECRET]"));
    assert!(!rendered.contains("supersecretvalue"));
    assert!(!rendered.contains("abcdefghijklmnopqrstuvwxyz"));
}

#[test]
fn agent_markdown_stream_sanitizes_terminal_controls() {
    let mut stream = AgentMarkdownStreamState::new(std::path::PathBuf::from("."));

    stream.push_delta("First\rSecond\u{1b}[31m red\u{1b}[0m\u{8}!");

    assert_eq!(stream.sanitized_raw_text(), "First\nSecond red!");
    let rendered = stream
        .display_lines()
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("First"));
    assert!(rendered.contains("Second red!"));
    assert!(!rendered.contains('\r'));
    assert!(!rendered.contains('\u{1b}'));
    assert!(!rendered.contains('\u{8}'));
}

#[test]
fn agent_markdown_stream_finalize_commits_partial_line() {
    let mut stream = AgentMarkdownStreamState::new(std::path::PathBuf::from("."));

    stream.push_delta("Partial answer without newline");
    stream.finalize_display_lines();

    let rendered = stream
        .display_lines()
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Partial answer without newline"));
    assert_eq!(stream.display_lines().len(), 1);
}

#[test]
fn agent_markdown_ingestion_and_borrowed_reads_do_not_repeat_work() {
    let mut stream = AgentMarkdownStreamState::new(std::path::PathBuf::from("."));
    for _ in 0..1000 {
        stream.push_delta("word ");
    }
    assert_eq!(stream.markdown_work().parses, 0);
    let first = stream.display_lines();
    let work = stream.markdown_work();
    let second = stream.display_lines();
    assert_eq!(first.as_ptr(), second.as_ptr());
    assert_eq!(stream.markdown_work(), work);
    assert_eq!(work.parses, 1);
}

#[test]
fn prioritizes_active_pending_interaction_in_ui_order() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.config = RaraConfig::default();
    app.snapshot = RuntimeSnapshot {
        pending_interactions: vec![
            PendingInteractionSnapshot {
                kind: InteractionKind::RequestInput,
                title: "Question".to_string(),
                summary: String::new(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: Some("plan_agent".to_string()),
                created_at_epoch_seconds: None,
            },
            PendingInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: "Pending Approval".to_string(),
                summary: "run cargo test".to_string(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: None,
                created_at_epoch_seconds: None,
            },
            PendingInteractionSnapshot {
                kind: InteractionKind::PlanApproval,
                title: "Plan Ready".to_string(),
                summary: "Review the plan.".to_string(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: None,
                created_at_epoch_seconds: None,
            },
        ],
        ..RuntimeSnapshot::default()
    };

    let active = app
        .active_pending_interaction()
        .expect("pending interaction");
    assert_eq!(active.kind, ActivePendingInteractionKind::PlanApproval);
    assert_eq!(active._snapshot.title, "Plan Ready");
}

#[test]
fn clear_pending_command_approval_removes_only_shell_approval() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.config = RaraConfig::default();
    app.snapshot = RuntimeSnapshot {
        pending_interactions: vec![
            PendingInteractionSnapshot {
                kind: InteractionKind::RequestInput,
                title: "Question".to_string(),
                summary: "Need a value".to_string(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: Some("worker".to_string()),
                created_at_epoch_seconds: None,
            },
            PendingInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: "Pending Approval".to_string(),
                summary: "run cargo test".to_string(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: None,
                created_at_epoch_seconds: None,
            },
            PendingInteractionSnapshot {
                kind: InteractionKind::PlanApproval,
                title: "Plan Ready".to_string(),
                summary: "Review the plan.".to_string(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: None,
                created_at_epoch_seconds: None,
            },
        ],
        ..RuntimeSnapshot::default()
    };

    assert!(app.pending_command_approval().is_some());

    app.clear_pending_command_approval();

    assert!(app.pending_command_approval().is_none());
    assert_eq!(app.snapshot.pending_interactions.len(), 2);
    assert!(
        app.snapshot
            .pending_interactions
            .iter()
            .any(|item| item.kind == InteractionKind::RequestInput)
    );
    assert!(
        app.snapshot
            .pending_interactions
            .iter()
            .any(|item| item.kind == InteractionKind::PlanApproval)
    );
}

#[test]
fn showing_plan_approval_resets_the_shared_action_selection() {
    let dir = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.approval_picker_idx = 3;

    app.show_pending_plan_approval(None);

    assert_eq!(app.approval_picker_idx, 0);
}

#[test]
fn sync_snapshot_reports_effective_network_access_for_pending_approval() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let rara_dir = root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    let mut app = TuiApp::new(ConfigManager {
        path: root.join("config.json"),
    })
    .expect("app");
    let mut agent = Agent::new(
        ToolManager::new(),
        std::sync::Arc::new(MockLlm),
        std::sync::Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        std::sync::Arc::new(SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        }),
        std::sync::Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
    );
    agent.pending_approval = Some(PendingApproval {
        tool_use_id: "tool-1".to_string(),
        request: BashCommandInput {
            command: Some("cargo check".to_string()),
            allow_net: false,
            ..Default::default()
        },
    });

    app.apply_runtime_snapshot(&agent, RuntimeExtensionSnapshot::default());

    let approval = app
        .pending_command_approval()
        .and_then(|interaction| interaction.approval.as_ref())
        .expect("pending approval");
    assert!(approval.allow_net);
}

#[tokio::test]
async fn sync_snapshot_reports_registered_runtime_hooks() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let rara_dir = root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    let mut app = TuiApp::new(ConfigManager {
        path: root.join("config.json"),
    })
    .expect("app");
    let agent = Agent::new(
        ToolManager::new(),
        std::sync::Arc::new(MockLlm),
        std::sync::Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        std::sync::Arc::new(SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        }),
        std::sync::Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
    );
    let bus = std::sync::Arc::new(crate::runtime_event_bus::RuntimeEventBus::new(4));
    let runtime = std::sync::Arc::new(crate::hook_runtime::HookRuntime::new(bus));
    runtime.register(
        "plugin-pre-tool".into(),
        crate::runtime_control::HookLifecycle::PreToolUse,
        Box::new(|_| {}),
    );
    app.hook_runtime = Some(runtime);

    app.apply_runtime_snapshot(
        &agent,
        RuntimeExtensionSnapshot {
            hook_count: 1,
            ..RuntimeExtensionSnapshot::default()
        },
    );

    assert_eq!(app.snapshot.extension_hook_count, 1);
}

#[test]
fn sync_snapshot_uses_cached_agent_definition_records() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let rara_dir = root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    let agents_dir = rara_dir.join("agents");
    std::fs::create_dir_all(&agents_dir).expect("agents");
    let agent_path = agents_dir.join("status-cache-test.md");
    std::fs::write(
        &agent_path,
        r#"---
name: status-cache-before
description: Cached before sync.
---

Cached prompt.
"#,
    )
    .expect("agent definition");
    let mut app = TuiApp::new(ConfigManager {
        path: root.join("config.json"),
    })
    .expect("app");
    let agent = Agent::new(
        ToolManager::new(),
        std::sync::Arc::new(MockLlm),
        std::sync::Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        std::sync::Arc::new(SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        }),
        std::sync::Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
    );
    std::fs::write(
        &agent_path,
        r#"---
name: status-cache-after
description: Should require a runtime rebuild.
---

Reloaded prompt.
"#,
    )
    .expect("updated agent definition");

    app.apply_runtime_snapshot(
        &agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(&agent, 0),
    );

    assert!(
        app.snapshot
            .extension_agent_status_lines
            .iter()
            .any(|line| {
                line.contains("status-cache-before")
                    && line.contains(".rara/agents/status-cache-test.md")
            })
    );
    assert!(
        !app.snapshot
            .extension_agent_status_lines
            .iter()
            .any(|line| line.contains("status-cache-after"))
    );
}

#[test]
fn sync_snapshot_counts_runtime_agent_definition_records() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().join("workspace");
    let rara_dir = root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    let home_root = dir.path().join("home");
    let mut app = TuiApp::new(ConfigManager {
        path: root.join("config.json"),
    })
    .expect("app");
    let mut agent = Agent::new(
        ToolManager::new(),
        std::sync::Arc::new(MockLlm),
        std::sync::Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        std::sync::Arc::new(SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        }),
        std::sync::Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir)),
    );
    agent.agent_definitions = AgentDefinitionCache::from_records_for_test(vec![
        AgentDefinitionLoadRecord {
            id: "repo-agent".to_string(),
            source_path: root.join(".rara").join("agents").join("repo-agent.md"),
            definition: None,
            error: Some("parse error".to_string()),
        },
        AgentDefinitionLoadRecord {
            id: "home-agent".to_string(),
            source_path: home_root.join(".rara").join("agents").join("home-agent.md"),
            definition: None,
            error: Some("parse error".to_string()),
        },
    ]);

    app.apply_runtime_snapshot(
        &agent,
        crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(&agent, 0),
    );

    assert_eq!(app.snapshot.extension_agent_count, 2);
    assert!(
        app.snapshot
            .extension_agent_status_lines
            .iter()
            .any(|line| line.contains("repo-agent"))
    );
    assert!(
        app.snapshot
            .extension_agent_status_lines
            .iter()
            .any(|line| line.contains("home-agent"))
    );
}

#[test]
fn parse_repo_slug_supports_common_github_remote_forms() {
    assert_eq!(
        parse_repo_slug("git@github.com:hawkingrei/rara.git").as_deref(),
        Some("hawkingrei/rara")
    );
    assert_eq!(
        parse_repo_slug("https://github.com/hawkingrei/rara.git").as_deref(),
        Some("hawkingrei/rara")
    );
    assert_eq!(
        parse_repo_slug("ssh://git@github.com/hawkingrei/rara.git").as_deref(),
        Some("hawkingrei/rara")
    );
}

#[test]
fn new_does_not_detect_repo_context_synchronously() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let app = TuiApp::new(cm).expect("app");

    assert!(app.repo_context_task.is_none());
    assert!(app.repo_slug.is_none());
    assert!(app.current_pr_url.is_none());
}

#[test]
fn new_starts_without_explicit_plugin_dirs() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let app = TuiApp::new(cm).expect("app");

    assert!(app.explicit_plugin_dirs.is_empty());
}

#[test]
fn push_entry_keeps_manual_transcript_scroll_position() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.transcript_scroll.update_layout(TranscriptScrollLayout {
        width: 80,
        height: 5,
        content_rows: 20,
    });
    app.transcript_scroll.scroll(-6);
    let scroll = app.transcript_scroll;

    app.push_entry(MessageRole::System, "background update");

    assert_eq!(app.transcript_scroll, scroll);
}

#[test]
fn finalize_agent_stream_keeps_manual_transcript_scroll_position() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.transcript_scroll.update_layout(TranscriptScrollLayout {
        width: 80,
        height: 5,
        content_rows: 20,
    });
    app.transcript_scroll.scroll(-4);
    let scroll = app.transcript_scroll;
    app.active_turn.entries.push(TranscriptEntry {
        role: MessageRole::Agent,
        message: "draft".into(),
        payload: None,
    });

    app.finalize_agent_stream(Some("final answer".into()));

    assert_eq!(app.transcript_scroll, scroll);
    assert_eq!(
        app.active_turn
            .entries
            .last()
            .map(|entry| entry.message.as_str()),
        Some("final answer")
    );
}

#[test]
fn queued_follow_up_messages_preserve_fifo_order() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    assert_eq!(app.queue_follow_up_message("first"), 1);
    assert_eq!(app.queue_follow_up_message("second"), 2);
    assert_eq!(app.queued_follow_up_preview(), Some("first"));
    assert_eq!(app.pop_queued_follow_up_message().as_deref(), Some("first"));
    assert_eq!(
        app.pop_queued_follow_up_message().as_deref(),
        Some("second")
    );
    assert_eq!(app.pop_queued_follow_up_message(), None);
}

#[test]
fn drain_queued_follow_up_messages_preserves_fifo_order() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.queue_follow_up_message("first");
    app.queue_follow_up_message("second");

    assert_eq!(
        app.drain_queued_follow_up_messages(),
        vec!["first".to_string(), "second".to_string()]
    );
    assert_eq!(app.pop_queued_follow_up_message(), None);
}

#[test]
fn pending_follow_up_messages_release_on_tool_boundary() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.begin_running_turn();
    assert_eq!(
        app.queue_follow_up_message_after_next_tool_boundary("first pending"),
        1
    );
    assert_eq!(app.pending_follow_up_preview(), Some("first pending"));
    assert_eq!(app.queued_end_of_turn_preview(), None);

    app.advance_running_tool_boundary();

    assert_eq!(app.pending_follow_up_preview(), None);
    assert_eq!(app.queued_end_of_turn_preview(), Some("first pending"));
    assert_eq!(
        app.pop_queued_follow_up_message().as_deref(),
        Some("first pending")
    );
}
