// Claude-style /status display — clean, sectioned, color-styled output.
//
// Each line is a ratatui Line with Span-styled values so colors
// actually render in the TUI, not just plain text.
use std::sync::atomic::Ordering;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::state::{StatusTab, TuiApp};

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

fn format_metric(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}K", n / 1_000)
    } else {
        n.to_string()
    }
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
