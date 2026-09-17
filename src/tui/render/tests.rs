use std::path::Path;

use insta::assert_snapshot;
use ratatui::style::Color;
use ratatui::text::Line;
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
use tempfile::tempdir;

use super::cells::HistoryCell;
use super::viewport::TranscriptViewport;
use super::{
    committed_turn_cell, compact_progress_summary_lines, compact_recent_first_summary_lines,
    compact_summary_text, current_turn_exploration_summary_from_entries, current_turn_tool_summary,
    desired_bottom_pane_height, desired_viewport_height, display_directory_for_startup,
    formatted_message_lines, prefixed_message_lines, renderable_transcript_lines,
    tool_action_label, transcript_scroll_offset, transcript_viewport, transcript_visual_row_count,
};
use crate::config::{ConfigManager, OpenAiEndpointKind, RaraConfig};
use crate::tools::bash::BashCommandInput;
use crate::tui::custom_terminal::Frame;
use crate::tui::state::SkillPickerEntry;
use crate::tui::state::{
    ApiKeyTarget, InteractionKind, ListPickerKind, Overlay, PendingApprovalSnapshot,
    PendingInteractionSnapshot, PlanningApprovalStatus, PlanningLifecycleSnapshot, ProviderFamily,
    RuntimeSnapshot, StatusTab, ToolTranscriptPayload, ToolTranscriptStatus, TranscriptEntry,
    TranscriptEntryPayload, TranscriptTurn, TuiApp,
};

fn provider_family_idx(family: ProviderFamily) -> usize {
    crate::tui::state::PROVIDER_FAMILIES
        .iter()
        .position(|(candidate, _, _)| *candidate == family)
        .expect("provider family present")
}

mod transcript_and_layout;

#[test]
fn ssh_startup_page_warns_without_opening_setup_window() {
    let temp = tempdir().expect("tempdir");
    let _ssh_env = crate::tui::terminal_ui::test_env::set_ssh_session(true);

    let cm = ConfigManager {
        path: temp.path().join("config.json"),
    };
    let mut config = RaraConfig::default();
    config.set_provider("openai-compatible");
    config.clear_api_key();
    cm.save(&config).expect("save config");

    let mut app = TuiApp::new(cm).expect("build tui app");
    app.snapshot.cwd = "~/devel/opensource/rara".into();
    assert!(app.overlay.is_none());

    let rendered = render_screen_text(&mut app, 100, 24);
    assert_snapshot!("ssh_startup_warning_screen", rendered);
}

struct ScopedEnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl ScopedEnvGuard {
    fn remove(vars: &[&str]) -> Self {
        let saved = vars
            .iter()
            .map(|v| (v.to_string(), std::env::var(v).ok()))
            .collect();
        for v in vars {
            unsafe { std::env::remove_var(v) };
        }
        Self { saved }
    }

    fn set(vars: &[(&str, &str)]) -> Self {
        let keys: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
        let guard = Self::remove(&keys);
        for (k, v) in vars {
            unsafe { std::env::set_var(k, v) };
        }
        guard
    }
}

impl Drop for ScopedEnvGuard {
    fn drop(&mut self) {
        for (var, val) in &self.saved {
            if let Some(v) = val {
                unsafe { std::env::set_var(var, v) };
            } else {
                unsafe { std::env::remove_var(var) };
            }
        }
    }
}

#[test]
fn provider_picker_renders_as_full_overlay_on_standard_terminal() {
    let temp = tempdir().expect("tempdir");
    // Scrub all API-key env vars so the snapshot is deterministic regardless
    // of developer machine or CI environment.
    // Redirect HOME to temp dir so OAuthManager (which reads ~/.rara/codex-auth/)
    // finds no saved Codex OAuth tokens from the developer's real home.
    let _home_guard = ScopedEnvGuard::set(&[("HOME", &temp.path().to_string_lossy())]);
    let _guard = ScopedEnvGuard::remove(&[
        "CODEX_API_KEY",
        "DEEPSEEK_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
    ]);
    let cm = ConfigManager {
        path: temp.path().join("config.json"),
    };
    let mut config = RaraConfig::default();
    config.clear_api_key();
    cm.save(&config).expect("save config");

    let mut app = TuiApp::new(cm).expect("build tui app");
    app.snapshot.cwd = "<CWD>".into();
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Provider));

    let rendered = render_screen_text(&mut app, 100, 24);
    assert_snapshot!("provider_picker_standard_terminal", rendered);
}

#[test]
fn openai_model_picker_renders_profile_manager_not_endpoint_presets() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "openrouter-main",
        "OpenRouter Main",
        OpenAiEndpointKind::Openrouter,
    );
    app.config
        .set_model(Some("anthropic/claude-3.7-sonnet".to_string()));
    app.config.set_api_key("sk-openrouter");
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let rendered = render_screen_text(&mut app, 100, 24);
    assert!(rendered.contains("Model Picker"));
    assert!(rendered.contains("Select a model"));
    assert!(!rendered.contains("DeepSeek (openai-compatible/deepseek-chat)"));
    assert!(!rendered.contains("Moonshot AI (openai-compatible/kimi-k2.6)"));
    assert!(!rendered.contains("OpenRouter (openai-compatible/openai/gpt-4o-mini)"));
}

#[test]
fn deepseek_model_picker_renders_catalog_models_and_refresh_hint() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_api_key("sk-deepseek");
    app.set_deepseek_model_options(vec![
        "deepseek-chat".to_string(),
        "deepseek-reasoner".to_string(),
    ]);
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let rendered = render_screen_text(&mut app, 100, 24);
    assert!(rendered.contains("Model Picker"));
    assert!(rendered.contains("Select a model"));
    assert!(rendered.contains("deepseek-chat"));
}

#[test]
fn openai_model_picker_renders_profile_defaults_when_fields_are_empty() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.config.select_openai_profile(
        "custom-defaults",
        "Custom Defaults",
        OpenAiEndpointKind::Custom,
    );
    let profile = app
        .config
        .openai_profiles
        .get_mut("custom-defaults")
        .expect("custom profile present");
    profile.model = None;
    profile.base_url = None;
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Model));

    let rendered = render_screen_text(&mut app, 100, 24);
    assert!(rendered.contains("Model Picker"));
    assert!(rendered.contains("Select a model"));
    assert!(rendered.contains("Select Profile"));
}

#[test]
fn command_palette_query_uses_full_width_without_leaking_bottom_status() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.bottom_pane.input = "/m".into();
    app.open_overlay(Overlay::CommandPalette);

    let rendered = render_screen_text(&mut app, 107, 53);
    assert!(rendered.contains("/model"));
    assert!(!rendered.contains("ctx~="));
    assert!(!rendered.contains("enter run  esc close"));
}

#[test]
fn command_palette_empty_query_does_not_render_inline_footer_hint() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.bottom_pane.input = "/".into();
    app.open_overlay(Overlay::CommandPalette);

    let rendered = render_screen_text(&mut app, 107, 53);
    assert!(rendered.contains("/approval"));
    assert!(rendered.contains("/model"));
    assert!(!rendered.contains("enter run  esc close"));
    assert!(!rendered.contains("up/down move  enter run  esc close"));
}

#[test]
fn api_key_editor_renders_full_prompt_on_standard_terminal() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let mut config = RaraConfig::default();
    config.set_provider("openai-compatible");
    config.base_url = Some("https://api.deepseek.com".into());
    config.model = Some("deepseek-chat".into());
    app.config = config;
    app.provider_picker_idx = provider_family_idx(ProviderFamily::OpenAiCompatible);
    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::OpenAiCompatible));

    let rendered = render_screen_text(&mut app, 100, 24);
    assert_snapshot!("api_key_editor_standard_terminal", rendered);
}

#[test]
fn deepseek_api_key_editor_uses_deepseek_copy() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::DeepSeek);
    app.config
        .select_openai_profile("deepseek-default", "DeepSeek", OpenAiEndpointKind::Deepseek);
    app.config.set_api_key("sk-deepseek");
    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::DeepSeek));

    let rendered = render_screen_text(&mut app, 100, 24);
    assert!(rendered.contains("DeepSeek API Key"));
    assert!(rendered.contains("Paste a DeepSeek API key"));
    assert!(rendered.contains("Enter save and load models"));
    assert!(rendered.contains("Esc back to model picker"));
    assert!(!rendered.contains("Codex API Key"));
    assert!(!rendered.contains("Esc back to login guide"));
}

#[test]
fn moonshot_api_key_editor_uses_explicit_target_when_codex_is_active() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.config.set_provider("codex");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::Kimi);
    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::Kimi));

    let rendered = render_screen_text(&mut app, 100, 24);
    assert!(rendered.contains("Moonshot AI API Key"));
    assert!(rendered.contains("Paste a Moonshot AI API key"));
    assert!(rendered.contains("Enter save and load models"));
    assert!(!rendered.contains("Codex API Key"));
    assert!(!rendered.contains("Paste a Codex API key"));
}

#[test]
fn kimi_coding_api_key_editor_names_the_dedicated_credential_domain() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.config.set_provider("codex");
    app.provider_picker_idx = provider_family_idx(ProviderFamily::KimiCoding);
    app.open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::KimiCoding));

    let rendered = render_screen_text(&mut app, 100, 24);
    assert!(rendered.contains("Kimi For Coding API Key"));
    assert!(rendered.contains("Paste a Kimi Code API key"));
    assert!(rendered.contains("dedicated Kimi coding endpoint"));
    assert!(!rendered.contains("Moonshot AI API Key"));
    assert!(!rendered.contains("Codex API Key"));
}

#[test]
fn skills_picker_renders_selected_entry_scope_and_scrolls() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.skill_picker_entries = (0..16)
        .map(|idx| SkillPickerEntry {
            name: format!("skill-{idx:02}"),
            title: format!("Skill {idx:02}"),
            scope: if idx % 2 == 0 { "repo" } else { "home" }.to_string(),
            disable_model_invocation: idx % 3 == 0,
        })
        .collect();
    app.open_overlay(Overlay::SkillsPicker);
    app.skill_picker_idx = 14;

    let rendered = render_screen_text(&mut app, 80, 16);
    assert!(
        rendered.contains("[auto] skill-14 [repo]"),
        "rendered:\n{rendered}"
    );
    assert!(rendered.contains("Skill 14"), "rendered:\n{rendered}");
    assert!(!rendered.contains("skill-00"));
}

fn render_screen_text(app: &mut TuiApp, width: u16, height: u16) -> String {
    let buffer = render_screen_buffer(app, width, height);

    (0..height)
        .map(|y| {
            let mut line = String::new();
            for x in 0..width {
                line.push_str(buffer[(x, y)].symbol());
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_screen_buffer(app: &mut TuiApp, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let mut frame = Frame {
        cursor_position: None,
        viewport_area: area,
        buffer: &mut buffer,
    };
    super::render(&mut frame, app);
    buffer
}

#[test]
fn prefixed_message_lines_keep_first_and_latest_lines() {
    let rendered = prefixed_message_lines(
        "Tool",
        &["intro", "middle 1", "middle 2", "latest 1", "latest 2"].join("\n"),
        3,
    )
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>();
    assert_eq!(rendered[0], "⚙ intro");
    assert_eq!(rendered[1], "  ... 2 more line(s)");
    assert_eq!(rendered[2], "  latest 1");
    assert_eq!(rendered[3], "  latest 2");
}

#[test]
fn prefixed_message_lines_show_truncation_when_max_lines_is_one() {
    let tool_rendered = prefixed_message_lines("Tool", &["intro", "latest 1"].join("\n"), 1)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(tool_rendered[0], "⚙ intro");
    assert!(tool_rendered[1].contains("more line"));
    assert_eq!(tool_rendered.len(), 2);

    // Second call with same arguments — should be identical.
    let tool_rendered2 = prefixed_message_lines("Tool", &["intro", "latest 1"].join("\n"), 1)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(tool_rendered2[0], "⚙ intro");
    assert!(tool_rendered2[1].contains("more line"));
    assert_eq!(tool_rendered2.len(), 2);
}

#[test]
fn formatted_agent_markdown_keeps_first_and_latest_lines() {
    let rendered = formatted_message_lines(
        "Agent",
        &["first line", "middle 1", "middle 2", "latest 1", "latest 2"].join("\n"),
        3,
        Some(Path::new(".")),
    )
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.contains("first line")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("... 2 more line(s)"))
    );
    assert!(rendered.iter().any(|line| line.contains("latest 1")));
    assert!(rendered.iter().any(|line| line.contains("latest 2")));
    assert!(!rendered.iter().any(|line| line.contains("middle 1")));
}

#[test]
fn formatted_agent_markdown_sanitizes_terminal_controls() {
    let rendered = formatted_message_lines(
        "Agent",
        "Again\rcommit-to-main\u{1b}[31m red\u{1b}[0m\u{8}!",
        10,
        Some(Path::new(".")),
    )
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");

    assert!(rendered.contains("Again"));
    assert!(rendered.contains("commit-to-main red!"));
    assert!(!rendered.contains('\r'));
    assert!(!rendered.contains('\u{1b}'));
    assert!(!rendered.contains('\u{8}'));
}

#[test]
fn context_overlay_snapshot_with_typical_budget() {
    use crate::context::ContextAssemblyEntry;
    use crate::tui::context_display::render_context_lines;

    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        cwd: "/workspace/rara".into(),
        branch: "main".into(),
        session_id: "session-abc".into(),
        history_len: 42,
        estimated_history_tokens: 12_000,
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
        planning_lifecycle: PlanningLifecycleSnapshot {
            plan_path: Some(".rara/sessions/session-abc/plan.md".into()),
            approval_status: PlanningApprovalStatus::Pending,
            tool_use_id: Some("exit-plan-abc".into()),
            ..PlanningLifecycleSnapshot::default()
        },
        plan_steps: vec![("pending".into(), "Implement /context".into())],
        plan_explanation: Some("Adding Claude Code-style context display".into()),
        assembly_entries: vec![
            ContextAssemblyEntry {
                cache_status: None,
                order: 1,
                layer: "stable_instructions".into(),
                kind: "project_instruction".into(),
                label: "AGENTS.md".into(),
                source_path: Some("AGENTS.md".into()),
                injected: true,
                inclusion_reason: "workspace instruction discovery".into(),
                budget_impact_tokens: Some(240),
                dropped_reason: None,
            },
            ContextAssemblyEntry {
                cache_status: None,
                order: 2,
                layer: "active_memory_inputs".into(),
                kind: "workspace_memory".into(),
                label: "Project Memory".into(),
                source_path: Some(".rara/memory.md".into()),
                injected: true,
                inclusion_reason: "effective prompt includes memory".into(),
                budget_impact_tokens: Some(64),
                dropped_reason: None,
            },
        ],
        ..Default::default()
    };
    app.config
        .set_model(Some("anthropic/claude-sonnet-4".to_string()));

    let lines = render_context_lines(&app, 78);
    let rendered = lines
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<Vec<_>>()
                .join("")
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert_snapshot!("context_overlay_typical_budget", rendered);
}

#[test]
fn unified_model_picker_snapshot() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    app.snapshot.cwd = "/workspace/rara".into();

    // Add mock OpenAI profiles to see diversity in the unified list
    app.config.openai_profiles.insert(
        "custom-gpt".into(),
        crate::config::OpenAiEndpointProfile {
            id: "custom-gpt".into(),
            label: "My Custom GPT".into(),
            kind: crate::config::OpenAiEndpointKind::Custom,
            model: Some("gpt-custom".into()),
            base_url: Some("https://api.example.com".into()),
            api_key: None,
            ..Default::default()
        },
    );

    app.overlay = Some(Overlay::ListPicker(ListPickerKind::UnifiedModel));
    app.model_picker_idx = 0;

    let rendered = render_screen_text(&mut app, 80, 20);
    assert_snapshot!("unified_model_picker", rendered);
}
