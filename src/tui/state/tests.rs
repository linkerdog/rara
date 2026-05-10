use rara_memory::vectordb::VectorDB;
use rara_state::state_db::{PersistedCompactState, PersistedPromptRuntimeState, StateDb};
use rara_tools::tool::ToolManager;
use tempfile::tempdir;

use super::{
    ActivePendingInteractionKind, AgentMarkdownStreamState, InteractionKind, ListPickerKind,
    Overlay, PROVIDER_FAMILIES, PendingInteractionSnapshot, ProviderFamily, RuntimeSnapshot,
    TranscriptEntry, TranscriptTurn, TuiApp, input_requests_command_palette, parse_repo_slug,
    state_db_status_error,
};
use crate::agent::{Agent, PendingApproval};
use crate::codex_model_catalog::{CodexModelOption, CodexReasoningOption};
use crate::config::{ConfigManager, OpenAiEndpointKind, RaraConfig};
use crate::config::{DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_MODEL};
use crate::llm::MockLlm;
use crate::session::SessionManager;
use crate::tools::bash::BashCommandInput;
use crate::tui::command::palette_commands;
use crate::workspace::WorkspaceMemory;

fn provider_family_idx(family: ProviderFamily) -> usize {
    PROVIDER_FAMILIES
        .iter()
        .position(|(candidate, _, _)| *candidate == family)
        .expect("provider family present")
}

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
        .display_lines
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
            },
            PendingInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: "Pending Approval".to_string(),
                summary: "run cargo test".to_string(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: None,
            },
            PendingInteractionSnapshot {
                kind: InteractionKind::PlanApproval,
                title: "Plan Ready".to_string(),
                summary: "Review the plan.".to_string(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: None,
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
            },
            PendingInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: "Pending Approval".to_string(),
                summary: "run cargo test".to_string(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: None,
            },
            PendingInteractionSnapshot {
                kind: InteractionKind::PlanApproval,
                title: "Plan Ready".to_string(),
                summary: "Review the plan.".to_string(),
                options: Vec::new(),
                note: None,
                approval: None,
                source: None,
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
        std::sync::Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
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

    app.sync_snapshot(&agent);

    let approval = app
        .pending_command_approval()
        .and_then(|interaction| interaction.approval.as_ref())
        .expect("pending approval");
    assert!(approval.allow_net);
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
fn push_entry_keeps_manual_transcript_scroll_position() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.transcript_scroll = 6;

    app.push_entry("System", "background update");

    assert_eq!(app.transcript_scroll, 6);
}

#[test]
fn finalize_agent_stream_keeps_manual_transcript_scroll_position() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.transcript_scroll = 4;
    app.active_turn.entries.push(TranscriptEntry {
        role: "Agent".into(),
        message: "draft".into(),
        payload: None,
    });

    app.finalize_agent_stream(Some("final answer".into()));

    assert_eq!(app.transcript_scroll, 4);
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

#[test]
fn openai_compatible_preset_sets_default_connection_fields() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    assert_eq!(
        app.selected_provider_family(),
        ProviderFamily::OpenAiCompatible
    );

    app.select_local_model(0);

    assert_eq!(app.config.provider, "openai-compatible");
    assert_eq!(
        app.config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Custom)
    );
    assert_eq!(app.config.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(
        app.config.base_url.as_deref(),
        Some("https://api.openai.com/v1")
    );
    assert_eq!(app.config.revision, None);
}

#[test]
fn openai_compatible_preset_preserves_custom_model_name() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config.set_provider("openai-compatible");
    app.config.set_model(Some("custom-model".to_string()));
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);

    app.select_local_model(0);

    assert_eq!(app.config.provider, "openai-compatible");
    assert_eq!(app.config.model.as_deref(), Some("custom-model"));
}

#[test]
fn deepseek_family_selects_deepseek_profile_and_model() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.select_local_model(0);
    assert_eq!(
        app.config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Deepseek)
    );
    assert_eq!(
        app.config.base_url.as_deref(),
        Some("https://api.deepseek.com/v1")
    );
    assert_eq!(app.config.model.as_deref(), Some("deepseek-chat"));
}

#[test]
fn deepseek_catalog_options_keep_current_custom_model_selectable() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config
        .set_model(Some("deepseek-v4-preview".to_string()));

    app.set_deepseek_model_options(vec!["deepseek-chat".to_string()]);

    assert!(
        app.deepseek_model_options
            .iter()
            .any(|model| model == "deepseek-v4-preview")
    );
    assert_eq!(app.model_picker_idx, app.selected_preset_idx());
    assert_eq!(
        app.deepseek_model_options
            .get(app.model_picker_idx)
            .map(String::as_str),
        Some("deepseek-v4-preview")
    );
}

#[test]
fn model_routing_view_infers_deepseek_auxiliary_model() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_model(Some("deepseek-v4-pro".to_string()));

    let routing = app.model_routing_view();

    assert_eq!(routing.main_model, "deepseek-v4-pro");
    assert_eq!(routing.auxiliary_model, "deepseek-v4-flash");
    assert_eq!(routing.auxiliary_route, "provider_lite");
    assert_eq!(routing.auxiliary_source, "inferred");
    assert!(!routing.auxiliary_uses_main_model);
}

#[test]
fn model_routing_view_falls_back_to_main_model_without_helper() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config.set_provider("ollama");
    app.config.set_model(Some("qwen3".to_string()));

    let routing = app.model_routing_view();

    assert_eq!(routing.main_model, "qwen3");
    assert_eq!(routing.auxiliary_model, "qwen3");
    assert_eq!(routing.auxiliary_route, "fallback");
    assert_eq!(routing.auxiliary_source, "main_model");
    assert!(routing.auxiliary_uses_main_model);
}

#[test]
fn terminal_diagnostics_view_uses_live_tui_dimensions() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.terminal_width = 123;
    app.terminal_focused = false;

    let terminal = app.terminal_diagnostics_view();

    assert_eq!(terminal.width_columns, 123);
    assert!(!terminal.focused);
    assert!(!terminal.user_agent.is_empty());
    assert!(!terminal.history_mode.is_empty());
}

#[test]
fn codex_preset_keeps_the_codex_model_label() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = 0;
    app.set_codex_model_options(vec![CodexModelOption {
        id: DEFAULT_CODEX_MODEL.to_string(),
        model: DEFAULT_CODEX_MODEL.to_string(),
        label: "gpt-5.4".to_string(),
        description: "Latest frontier agentic coding model.".to_string(),
        reasoning_options: vec![CodexReasoningOption {
            value: "medium".to_string(),
            label: "Medium".to_string(),
            description: "Default reasoning effort.".to_string(),
            is_default: true,
        }],
        default_reasoning_effort: Some("medium".to_string()),
        is_default: true,
    }]);
    app.select_local_model(0);

    assert_eq!(app.config.provider, "codex");
    assert_eq!(app.config.model.as_deref(), Some(DEFAULT_CODEX_MODEL));
    assert_eq!(app.config.base_url.as_deref(), Some(DEFAULT_CODEX_BASE_URL));
}

#[test]
fn opening_openai_compatible_model_picker_restores_provider_scoped_state() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config.set_provider("openai-compatible");
    app.config
        .set_base_url(Some("http://proxy.local/v1".to_string()));
    app.config.set_model(Some("custom-model".to_string()));
    app.config.set_provider("codex");
    app.config.set_model(Some("codex".to_string()));

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    assert_eq!(app.config.provider, "openai-compatible");
    assert_eq!(
        app.config.base_url.as_deref(),
        Some("http://proxy.local/v1")
    );
    assert_eq!(app.config.model.as_deref(), Some("custom-model"));
}

#[test]
fn opening_openai_compatible_model_picker_excludes_deepseek_profile_kind() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_model(Some("deepseek-reasoner".to_string()));
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    assert_eq!(
        app.config.active_openai_profile_kind(),
        Some(OpenAiEndpointKind::Custom)
    );
    assert_eq!(app.config.model.as_deref(), Some("gpt-4o-mini"));
}

#[test]
fn openai_compatible_model_picker_selects_profile_rows() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    assert_eq!(app.current_model_picker_len(), 1);
    assert_eq!(app.model_picker_idx, 0);

    assert_eq!(
        app.selected_openai_model_picker_action(),
        Some(crate::tui::state::OpenAiModelPickerAction::SelectProfile)
    );

    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    assert_eq!(app.current_model_picker_len(), 2);

    app.model_picker_idx = 1;
    assert_eq!(
        app.selected_openai_model_picker_action(),
        Some(crate::tui::state::OpenAiModelPickerAction::SelectProfile)
    );
}

#[test]
fn openai_compatible_model_picker_deletes_active_profile_and_keeps_next() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "custom-default",
        "Custom endpoint",
        OpenAiEndpointKind::Custom,
    );
    app.config.select_openai_profile(
        "openrouter-default",
        "OpenRouter",
        OpenAiEndpointKind::Openrouter,
    );
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-default")
    );
    assert_eq!(
        app.delete_active_openai_profile().as_deref(),
        Some("OpenRouter")
    );
    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("custom-default")
    );
    assert_eq!(app.model_picker_idx, 0);
    assert_eq!(app.current_model_picker_len(), 1);
}

#[test]
fn openai_profile_active_state_survives_switching_to_codex_and_ollama() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-main",
        "OpenRouter Main",
        OpenAiEndpointKind::Openrouter,
    );
    app.config
        .set_model(Some("anthropic/claude-3.7-sonnet".to_string()));
    app.config.set_api_key("sk-openrouter");
    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-main")
    );

    app.provider_picker_idx = 0;
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    app.select_local_model(0);
    assert_eq!(app.config.provider, "codex");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::Ollama);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    app.select_local_model(0);
    assert_eq!(app.config.provider, "ollama");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-main")
    );
    assert_eq!(
        app.config.model.as_deref(),
        Some("anthropic/claude-3.7-sonnet")
    );
    assert_eq!(app.model_picker_idx, 0);
}

#[test]
fn opening_openai_profile_picker_prefers_active_profile_of_selected_kind() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config.select_openai_profile(
        "openrouter-main",
        "OpenRouter Main",
        OpenAiEndpointKind::Openrouter,
    );
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.model_picker_idx = 3;

    app.open_overlay(Overlay::ListPicker(ListPickerKind::OpenAiProfile));

    assert_eq!(
        app.selected_openai_profile_kind(),
        Some(OpenAiEndpointKind::Openrouter)
    );
    assert_eq!(app.openai_profile_picker_idx, 1);
}

#[test]
fn openai_model_selection_keeps_non_default_profile_for_same_kind() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-main",
        "OpenRouter Main",
        OpenAiEndpointKind::Openrouter,
    );

    app.select_local_model(3);

    assert_eq!(
        app.config.active_openai_profile_id(),
        Some("openrouter-main")
    );
    assert_eq!(
        app.config.active_openai_profile_label(),
        Some("OpenRouter Main")
    );
}

#[test]
fn model_name_editor_seeds_from_selected_provider_state() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.config.set_provider("openai-compatible");
    app.config.set_model(Some("custom-model".to_string()));
    app.config.set_provider("codex");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    app.open_overlay(Overlay::ModelNameEditor);

    assert_eq!(app.model_name_input, "custom-model");
}

#[test]
fn model_name_editor_does_not_panic_when_provider_has_no_presets() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    // DeepSeek has empty presets (&[])
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));
    app.open_overlay(Overlay::ModelNameEditor);

    // Should not panic, and model_name_input stays empty since
    // config.model is None and there is no preset to fall back to.
    assert_eq!(app.model_name_input, "");
}

#[test]
fn closing_auth_mode_picker_with_empty_stack_returns_to_none() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.open_overlay(Overlay::ListPicker(ListPickerKind::AuthMode));
    app.close_overlay();

    // Stack-based back-navigation: closing the only overlay returns to None.
    assert!(app.overlay.is_none());
}

#[test]
fn resume_picker_refreshes_recent_threads_on_open() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    let state_db = StateDb::new_for_root_dir(dir.path().join(".rara")).expect("state db");
    app.attach_state_db(std::sync::Arc::new(state_db));

    assert!(app.recent_threads.is_empty());

    app.state_db
        .as_ref()
        .expect("state db")
        .upsert_session(
            "thread-1",
            "/tmp/workspace",
            "main",
            "ollama",
            "qwen3",
            None,
            "execute",
            "always",
            None,
            &PersistedPromptRuntimeState::default(),
            1,
            0,
            &PersistedCompactState::default(),
        )
        .expect("upsert thread");

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));

    assert_eq!(app.recent_threads.len(), 1);
    assert_eq!(app.recent_threads[0].metadata.session_id, "thread-1");
    assert_eq!(app.resume_picker_idx, 0);
}

#[test]
fn finalize_agent_stream_updates_latest_committed_turn_when_final_text_arrives_late() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.committed_turns.push(TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "你好".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "你好！".into(),
                payload: None,
            },
        ],
    });

    app.finalize_agent_stream(Some("你好！有什么我可以帮你的？".into()));

    assert!(app.active_turn.entries.is_empty());
    assert_eq!(
        app.committed_turns
            .last()
            .and_then(|turn| turn.entries.last())
            .map(|entry| entry.message.as_str()),
        Some("你好！有什么我可以帮你的？")
    );
    assert_eq!(
        app.committed_turns.last().map(|turn| turn
            .entries
            .iter()
            .filter(|entry| entry.role == "Agent")
            .count()),
        Some(1)
    );
}

#[test]
fn streamed_agent_output_scrubs_internal_runtime_blocks_before_commit() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.append_agent_delta("Visible answer.\n");
    app.append_agent_delta("<agent_runtime>\n{\"phase\":\"tool_results_available\"}");
    let live_text = app
        .agent_stream_lines()
        .expect("agent stream")
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(live_text.contains("Visible answer."));
    assert!(!live_text.contains("agent_runtime"));
    assert!(!live_text.contains("tool_results_available"));

    app.append_agent_delta("\n</agent_runtime>\nFinal answer.");
    app.finalize_agent_stream(None);

    let message = app
        .active_turn
        .entries
        .iter()
        .find(|entry| entry.role == "Agent")
        .map(|entry| entry.message.as_str())
        .expect("agent message");
    assert!(message.contains("Visible answer."));
    assert!(message.contains("Final answer."));
    assert!(!message.contains("agent_runtime"));
    assert!(!message.contains("tool_results_available"));
}

#[test]
fn streamed_agent_output_appends_visible_text_after_internal_block() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.append_agent_delta("Visible before.\n");
    app.append_agent_delta("<agent_runtime>\nhidden");
    app.append_agent_delta("\n</agent_runtime>\nVisible after.");

    let live_text = app
        .agent_stream_lines()
        .expect("agent stream")
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(live_text.matches("Visible before.").count(), 1);
    assert_eq!(live_text.matches("Visible after.").count(), 1);
    assert!(!live_text.contains("agent_runtime"));
    assert!(!live_text.contains("hidden"));
}

#[test]
fn finalized_agent_stream_does_not_replace_agent_text_before_tool_boundary() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.push_entry("You", "Fix the rendering order");

    app.append_agent_delta("First assistant segment.");
    app.finalize_agent_stream(None);
    app.push_entry("Running", "Run cargo check");
    app.append_agent_delta("Second assistant segment.");
    app.finalize_agent_stream(None);

    let agent_entries = app
        .active_turn
        .entries
        .iter()
        .filter(|entry| entry.role == "Agent")
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        agent_entries,
        vec!["First assistant segment.", "Second assistant segment."]
    );

    let first_agent = app
        .active_turn
        .entries
        .iter()
        .position(|entry| entry.message == "First assistant segment.")
        .unwrap();
    let running = app
        .active_turn
        .entries
        .iter()
        .position(|entry| entry.message == "Run cargo check")
        .unwrap();
    let second_agent = app
        .active_turn
        .entries
        .iter()
        .position(|entry| entry.message == "Second assistant segment.")
        .unwrap();
    assert!(first_agent < running);
    assert!(running < second_agent);
}

#[test]
fn flushed_agent_thinking_stream_scrubs_internal_runtime_blocks() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.append_agent_thinking_delta("Visible thought.\n");
    app.append_agent_thinking_delta("<agent_runtime>\n{\"phase\":\"tool_results_available\"}");
    app.append_agent_thinking_delta("\n</agent_runtime>\nNext thought.");
    app.flush_agent_thinking_stream_to_live_event();

    assert_eq!(app.active_live.events.len(), 1);
    let event = app.active_live.events.first().expect("thinking event");
    assert_eq!(event.role(), "Thinking");
    assert_eq!(event.message(), "Visible thought.\n\nNext thought.");
    assert!(!event.message().contains("agent_runtime"));
    assert!(!event.message().contains("tool_results_available"));
}

#[test]
fn live_progress_events_sanitize_terminal_controls() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.record_running_action("Run\r\u{1b}[31mcargo check\u{1b}[0m\u{8}");
    app.record_exploration_note("Read\tfile\u{7}");

    assert_eq!(app.active_live.events.len(), 2);
    assert_eq!(app.active_live.events[0].message(), "Run\ncargo check");
    assert_eq!(app.active_live.events[1].message(), "Read    file");
    assert!(!app.active_live.events[0].message().contains('\r'));
    assert!(!app.active_live.events[0].message().contains('\u{1b}'));
    assert!(!app.active_live.events[1].message().contains('\u{7}'));
}

#[test]
fn finalize_agent_stream_replaces_earlier_agent_entries_in_active_turn() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "你好".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "你好".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "System".into(),
                message: "temporary runtime detail".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "你好！".into(),
                payload: None,
            },
        ],
    };

    app.finalize_agent_stream(Some("你好！有什么我可以帮你的？".into()));

    let agent_entries = app
        .active_turn
        .entries
        .iter()
        .filter(|entry| entry.role == "Agent")
        .collect::<Vec<_>>();
    assert_eq!(agent_entries.len(), 1);
    assert_eq!(agent_entries[0].message, "你好！有什么我可以帮你的？");
}

#[test]
fn restore_committed_turns_sets_inserted_counter_to_match() {
    let dir = tempdir().unwrap();
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    // Simulate session resume: restore N turns that were already on screen.
    let turns = vec![
        TranscriptTurn {
            entries: vec![TranscriptEntry::new("You", "hello")],
        },
        TranscriptTurn {
            entries: vec![TranscriptEntry::new("Agent", "hi there")],
        },
        TranscriptTurn {
            entries: vec![TranscriptEntry::new("You", "bye")],
        },
    ];
    let n = turns.len();
    app.restore_committed_turns(turns);

    assert_eq!(app.committed_turns.len(), n);
    assert_eq!(app.active_turn.entries.len(), 0);
}

// ── Command palette selection persistence ──────────────────────────

/// Typing more characters while the palette is open should NOT reset
/// `command_palette_idx` back to 0.
#[test]
fn command_palette_selection_persists_while_typing() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.config = RaraConfig::default();

    // Open palette by typing slash
    app.insert_active_input_char('/');
    assert!(matches!(app.overlay, Some(Overlay::CommandPalette)));
    assert_eq!(app.command_palette_idx, 0);

    // Simulate arrow-down to move selection
    let cmd_count = palette_commands(&app, app.command_query()).len();
    assert!(cmd_count > 1, "need at least 2 commands for this test");
    app.command_palette_idx = 1;

    // Type more characters — this triggers sync_command_palette_with_input
    // which must NOT reset command_palette_idx when the palette is already open.
    app.insert_active_input_char('h');
    assert!(matches!(app.overlay, Some(Overlay::CommandPalette)));
    assert_eq!(
        app.command_palette_idx, 1,
        "selection idx should stay at 1 after typing more chars"
    );

    // Type another character — still should not reset
    app.insert_active_input_char('e');
    assert_eq!(
        app.command_palette_idx, 1,
        "selection idx should still be 1 after further typing"
    );
}

/// Closing the palette (Esc) should clear the slash input and reset
/// `command_palette_idx`.
#[test]
fn close_command_palette_clears_input_and_resets_idx() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.config = RaraConfig::default();

    // Open palette by typing slash
    app.insert_active_input_char('/');
    app.insert_active_input_char('h');
    app.insert_active_input_char('e');
    app.insert_active_input_char('l');
    assert!(matches!(app.overlay, Some(Overlay::CommandPalette)));
    assert!(!app.bottom_pane.input.is_empty());

    // Move selection
    app.command_palette_idx = 2;

    // Close the palette
    app.close_overlay();

    // After close: input should be cleared, idx reset
    assert!(app.bottom_pane.input.is_empty(), "input should be cleared");
    assert_eq!(
        app.command_palette_idx, 0,
        "command_palette_idx should reset to 0"
    );
    assert!(matches!(app.overlay, None), "overlay should be closed");
}

/// Clearing the slash prefix should close the palette and reset idx.
#[test]
fn clearing_slash_closes_palette() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.config = RaraConfig::default();

    // Open palette and move to index 2
    app.insert_active_input_char('/');
    app.command_palette_idx = 2;

    // Backspace to clear the slash — sync fires and closes the palette
    app.backspace_active_input();
    assert!(matches!(app.overlay, None));
    assert_eq!(app.command_palette_idx, 0);
    assert!(app.bottom_pane.input.is_empty());
}
