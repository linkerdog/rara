// Claude-style /status display — clean, sectioned, color-styled output.
//
// Each line is a ratatui Line with Span-styled values so colors
// actually render in the TUI, not just plain text.
use std::sync::atomic::Ordering;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::state::{PlanningApprovalStatus, StatusTab, TuiApp};

pub(crate) fn render_status_lines(app: &TuiApp, tab: StatusTab) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    match tab {
        StatusTab::Overview => render_overview_status(app, &mut lines),
        StatusTab::Config => render_config_status(app, &mut lines),
        StatusTab::Context => render_context_status(app, &mut lines),
    }

    lines
}

fn render_overview_status(app: &TuiApp, lines: &mut Vec<Line<'static>>) {
    section_header(lines, "Provider & Model");
    let routing = app.model_routing_view();
    kv(lines, "provider", &app.config.provider, Color::Cyan);
    kv(lines, "model", &routing.main_model, Color::LightBlue);
    kv(
        lines,
        "auxiliary",
        &format!("{} ({})", routing.auxiliary_model, routing.auxiliary_route),
        if routing.auxiliary_uses_main_model {
            Color::DarkGray
        } else {
            Color::LightBlue
        },
    );
    if app.config.provider == "openai-compatible" {
        kv(
            lines,
            "endpoint",
            app.config.active_openai_profile_label().unwrap_or("-"),
            Color::DarkGray,
        );
    }

    section_spacer(lines);
    section_header(lines, "Local Embeddings");
    render_local_embedding_status(app, lines);

    section_spacer(lines);
    section_header(lines, "Execution");
    kv(
        lines,
        "mode",
        app.agent_execution_mode_label(),
        Color::LightBlue,
    );
    kv(
        lines,
        "permissions",
        app.permission_mode_label(),
        Color::LightBlue,
    );
    kv(lines, "phase", app.runtime_phase_label(), Color::DarkGray);
    if let Some(detail) = &app.runtime_phase_detail {
        kv(lines, "detail", detail, Color::Gray);
    }
    kv(
        lines,
        "bash",
        app.bash_approval_mode_label(),
        Color::DarkGray,
    );

    section_spacer(lines);
    section_header(lines, "Planning");
    render_planning_lifecycle_status(app, lines);

    section_spacer(lines);
    section_header(lines, "Workspace");
    let snap = &app.snapshot;
    kv(lines, "dir", &home_path(&snap.cwd), Color::DarkGray);
    kv(lines, "branch", &snap.branch, Color::DarkGray);
    kv(lines, "session", &snap.session_id, Color::Gray);

    section_spacer(lines);
    section_header(lines, "Extensions");
    let skill_label = if snap.extension_skill_count == 0 {
        "0 loaded".to_string()
    } else {
        format!(
            "{} loaded ({})",
            snap.extension_skill_count,
            snap.extension_skill_scopes.join(", ")
        )
    };
    kv(
        lines,
        "skills",
        &skill_label,
        if snap.extension_skill_count > 0 {
            Color::LightBlue
        } else {
            Color::DarkGray
        },
    );
    kv(
        lines,
        "hooks",
        &snap.extension_hook_count.to_string(),
        Color::DarkGray,
    );
    kv(
        lines,
        "agents",
        &snap.extension_agent_count.to_string(),
        Color::DarkGray,
    );
    for line in &snap.extension_agent_status_lines {
        lines.push(Line::from(Span::styled(
            line.clone(),
            Style::default().fg(Color::DarkGray),
        )));
    }
}

fn render_config_status(app: &TuiApp, lines: &mut Vec<Line<'static>>) {
    section_header(lines, "API & Auth");
    let surface = app.config.effective_provider_surface();
    kv(
        lines,
        "base_url",
        surface.base_url.display_or("-"),
        Color::DarkGray,
    );
    kv(
        lines,
        "api_key",
        &api_key_label(app),
        if app.config.has_api_key() {
            Color::LightGreen
        } else {
            Color::Yellow
        },
    );
    kv(
        lines,
        "reasoning",
        &format!(
            "{} ({})",
            surface.reasoning_summary.display_or("auto"),
            surface.reasoning_summary.source.label()
        ),
        Color::DarkGray,
    );

    section_spacer(lines);
    section_header(lines, "Network & Sandbox");
    kv(lines, "sandbox", &sandbox_label(app), Color::LightBlue);
    kv(
        lines,
        "network",
        if app.sandbox_network_access.load(Ordering::Relaxed) {
            "permitted"
        } else {
            "restricted"
        },
        if app.sandbox_network_access.load(Ordering::Relaxed) {
            Color::Yellow
        } else {
            Color::LightGreen
        },
    );

    section_spacer(lines);
    section_header(lines, "Terminal");
    let terminal = app.terminal_diagnostics_view();
    kv(lines, "name", &terminal.name, Color::LightBlue);
    kv(lines, "user_agent", &terminal.user_agent, Color::DarkGray);
    kv(
        lines,
        "term",
        terminal.term.as_deref().unwrap_or("-"),
        Color::DarkGray,
    );
    kv(
        lines,
        "term_program",
        terminal.term_program.as_deref().unwrap_or("-"),
        Color::DarkGray,
    );
    kv(lines, "multiplexer", &terminal.multiplexer, Color::DarkGray);
    kv(lines, "remote", &terminal.remote, Color::DarkGray);
    kv(lines, "history", &terminal.history_mode, Color::DarkGray);
    kv(
        lines,
        "focused",
        if terminal.focused { "true" } else { "false" },
        Color::DarkGray,
    );
    kv(
        lines,
        "width",
        &format!("{} columns", terminal.width_columns),
        Color::DarkGray,
    );
}

fn render_context_status(app: &TuiApp, lines: &mut Vec<Line<'static>>) {
    section_header(lines, "Context Summary");
    let snap = &app.snapshot;
    kv(
        lines,
        "history",
        &format!("{} tokens", snap.estimated_history_tokens),
        Color::DarkGray,
    );
    if let Some(window) = snap.context_window_tokens {
        kv(
            lines,
            "window",
            &format_metric(window as u64),
            Color::LightBlue,
        );
    }
    if let Some(remaining) = snap.remaining_input_budget {
        kv(
            lines,
            "budget",
            &format!("{} tokens remaining", remaining),
            if remaining < 1024 {
                Color::Yellow
            } else {
                Color::DarkGray
            },
        );
    }
    if snap.total_input_tokens > 0 || snap.total_output_tokens > 0 {
        let hit = snap.total_cache_hit_tokens;
        let miss = snap.total_cache_miss_tokens;
        let total = hit + miss;
        let rate = if total > 0 {
            (hit as f64 / total as f64 * 100.0) as u32
        } else {
            0
        };
        kv(
            lines,
            "cache",
            &format!("{}% hit ({} hits / {} misses)", rate, hit, miss),
            Color::LightGreen,
        );
    }
    if snap.compaction_count > 0 {
        kv(
            lines,
            "compactions",
            &snap.compaction_count.to_string(),
            Color::DarkGray,
        );
    }

    kv(
        lines,
        "todo",
        &format!(
            "{} total, {} active, {} done",
            snap.todo.summary.total,
            snap.todo.summary.pending + snap.todo.summary.in_progress,
            snap.todo.summary.completed
        ),
        Color::LightBlue,
    );
    render_planning_lifecycle_summary(app, lines);

    section_spacer(lines);
    section_header(lines, "More Detail");
    kv(
        lines,
        "context",
        "open /context for layer detail",
        Color::Gray,
    );
}

// ── helpers ──

fn section_header(lines: &mut Vec<Line<'static>>, title: &str) {
    lines.push(Line::from(Span::styled(
        title.to_string(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
}

fn section_spacer(lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(""));
}

fn kv(lines: &mut Vec<Line<'static>>, key: &str, value: &str, value_color: Color) {
    let key_span = Span::styled(
        format!("  {key:<14} "),
        Style::default().fg(Color::DarkGray),
    );
    let value_span = Span::styled(value.to_string(), Style::default().fg(value_color));
    lines.push(Line::from(vec![key_span, value_span]));
}

fn render_local_embedding_status(app: &TuiApp, lines: &mut Vec<Line<'static>>) {
    let status = &app.local_model_server;
    let (label, color) = match status.state {
        crate::local_model_server::LocalModelServerState::Ready => ("ready", Color::LightGreen),
        crate::local_model_server::LocalModelServerState::Starting => ("starting", Color::Yellow),
        crate::local_model_server::LocalModelServerState::WaitingForServer => {
            ("waiting_for_server", Color::Yellow)
        }
        crate::local_model_server::LocalModelServerState::CreatingVenv => {
            ("creating_venv", Color::Yellow)
        }
        crate::local_model_server::LocalModelServerState::InstallingDependencies => {
            ("installing_dependencies", Color::Yellow)
        }
        crate::local_model_server::LocalModelServerState::PreparingModel => {
            ("preparing_model", Color::Yellow)
        }
        crate::local_model_server::LocalModelServerState::PreparedButStopped => {
            ("prepared_stopped", Color::Yellow)
        }
        crate::local_model_server::LocalModelServerState::SetupRequired => {
            ("setup_required", Color::Yellow)
        }
        crate::local_model_server::LocalModelServerState::Error => ("error", Color::Red),
    };
    kv(lines, "embedding", label, color);
    kv(lines, "backend", &status.backend, Color::DarkGray);
    kv(lines, "model", &status.model, Color::LightBlue);
    kv(lines, "detail", &status.detail, Color::DarkGray);
}

fn render_planning_lifecycle_status(app: &TuiApp, lines: &mut Vec<Line<'static>>) {
    let lifecycle = &app.snapshot.planning_lifecycle;
    kv(
        lines,
        "status",
        lifecycle.approval_status.label(),
        planning_status_color(lifecycle.approval_status),
    );
    kv(
        lines,
        "plan",
        lifecycle.plan_path.as_deref().unwrap_or("-"),
        Color::DarkGray,
    );
    kv(
        lines,
        "pending_age",
        lifecycle.pending_age_label(),
        Color::DarkGray,
    );
    kv(
        lines,
        "decision",
        lifecycle.last_decision_label(),
        Color::DarkGray,
    );
    kv(
        lines,
        "revision",
        lifecycle.approved_plan_revision_label(),
        Color::DarkGray,
    );
    if lifecycle.tool_use_id.is_some() {
        kv(
            lines,
            "exit_plan_tool",
            lifecycle.tool_use_id_label(),
            Color::DarkGray,
        );
    }
}

fn render_planning_lifecycle_summary(app: &TuiApp, lines: &mut Vec<Line<'static>>) {
    let lifecycle = &app.snapshot.planning_lifecycle;
    kv(
        lines,
        "planning",
        &format!(
            "status={} plan={} decision={} revision={}",
            lifecycle.approval_status.label(),
            lifecycle.plan_path.as_deref().unwrap_or("-"),
            lifecycle.last_decision_label(),
            lifecycle.approved_plan_revision_label(),
        ),
        planning_status_color(lifecycle.approval_status),
    );
}

fn planning_status_color(status: PlanningApprovalStatus) -> Color {
    match status {
        PlanningApprovalStatus::None => Color::DarkGray,
        PlanningApprovalStatus::Pending => Color::Yellow,
        PlanningApprovalStatus::Approved => Color::LightGreen,
        PlanningApprovalStatus::Revising => Color::LightBlue,
        PlanningApprovalStatus::Rejected => Color::Red,
    }
}

fn format_metric(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}K", n / 1_000)
    } else {
        n.to_string()
    }
}

/// Shared token count formatter: "1.0k", "2.5M", or plain number.
/// Keep format stable — sidebar, footer, and context display all use this.
pub(crate) fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

/// One-line context summary used by sidebar.
pub(crate) fn context_sidebar_summary(snap: &crate::tui::state::RuntimeSnapshot) -> String {
    let used = snap
        .stable_instructions_budget
        .saturating_add(snap.workspace_prompt_budget)
        .saturating_add(snap.active_turn_budget)
        .saturating_add(snap.compacted_history_budget)
        .saturating_add(snap.retrieved_memory_budget)
        .saturating_add(snap.reserved_output_tokens);
    let token_label = format_token_count(used);
    let mut parts = vec![token_label];
    if let Some(window) = snap.context_window_tokens {
        parts.push(format_token_count(window));
    }
    if snap.history_len > 0 {
        parts.push(format!("{} turns", snap.history_len));
    }
    if snap.compaction_count > 0 {
        parts.push(format!("compacted {}×", snap.compaction_count));
    }
    parts.join(" · ")
}

fn home_path(cwd: &str) -> String {
    if let Ok(home) = std::env::var("HOME")
        && let Some(stripped) = cwd.strip_prefix(&home)
    {
        return format!("~{}", stripped);
    }
    cwd.to_string()
}

fn api_key_label(app: &TuiApp) -> String {
    if app.config.has_api_key() {
        let source = app
            .config
            .effective_provider_surface()
            .api_key
            .source
            .label();
        format!("●●●●● ({source})")
    } else {
        "not set".to_string()
    }
}

fn sandbox_label(_app: &TuiApp) -> String {
    if cfg!(target_os = "macos") {
        "macos-seatbelt"
    } else if cfg!(target_os = "linux") {
        "linux-bubblewrap"
    } else {
        "none"
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::render_status_lines;
    use crate::config::ConfigManager;
    use crate::tui::state::{
        PlanningApprovalStatus, PlanningLifecycleSnapshot, RuntimeSnapshot, StatusTab, TuiApp,
    };

    #[test]
    fn overview_status_reports_local_embedding_component() {
        let temp = tempdir().expect("tempdir");
        let app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");

        let rendered = render_status_lines(&app, StatusTab::Overview)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Local Embeddings"));
        assert!(rendered.contains("setup_required"));
        assert!(rendered.contains("embedding"));
    }

    #[test]
    fn overview_status_reports_agent_extension_details() {
        let temp = tempdir().expect("tempdir");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.snapshot = RuntimeSnapshot {
            extension_agent_count: 1,
            extension_agent_status_lines: vec![
                "  code-reviewer  .rara/agents/code-reviewer.md  ok  (disabled)".to_string(),
            ],
            ..RuntimeSnapshot::default()
        };

        let rendered = render_status_lines(&app, StatusTab::Overview)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("agents"));
        assert!(rendered.contains("code-reviewer"));
        assert!(rendered.contains(".rara/agents/code-reviewer.md"));
    }

    #[test]
    fn overview_status_reports_planning_lifecycle() {
        let temp = tempdir().expect("tempdir");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.snapshot = RuntimeSnapshot {
            planning_lifecycle: PlanningLifecycleSnapshot {
                plan_path: Some(".rara/sessions/session-123/plan.md".into()),
                approval_status: PlanningApprovalStatus::Pending,
                tool_use_id: Some("exit-tool-123".into()),
                ..PlanningLifecycleSnapshot::default()
            },
            ..RuntimeSnapshot::default()
        };

        let rendered = render_status_lines(&app, StatusTab::Overview)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Planning\n"));
        assert!(rendered.contains("status         pending"));
        assert!(rendered.contains(".rara/sessions/session-123/plan.md"));
        assert!(rendered.contains("exit-tool-123"));
    }
}
