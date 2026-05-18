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
use crate::tui::custom_terminal::Frame;
use crate::tui::state::{
    ListPickerKind, Overlay, ProviderFamily, RuntimeSnapshot, StatusTab, TranscriptEntry,
    TranscriptTurn, TuiApp,
};

fn provider_family_idx(family: ProviderFamily) -> usize {
    crate::tui::state::PROVIDER_FAMILIES
        .iter()
        .position(|(candidate, _, _)| *candidate == family)
        .expect("provider family present")
}

#[test]
fn committed_turn_does_not_truncate_agent_response() {
    let entries = vec![
        TranscriptEntry {
            role: "You".into(),
            message: "Review the code".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Agent".into(),
            message: (1..=12)
                .map(|idx| format!("Line {idx}"))
                .collect::<Vec<_>>()
                .join("\n"),
            payload: None,
        },
    ];

    let rendered = committed_turn_cell(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Line 12"));
    assert!(!rendered.contains("more line(s)"));
}

#[test]
fn keeps_history_reserve_once_transcript_exists() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.committed_turns.push(TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Earlier prompt".into(),
            payload: None,
        }],
    });

    let height = desired_viewport_height(&app, 120, 24);
    assert!(height > 5);
    assert!(height < 24);
}

#[test]
fn startup_viewport_uses_full_height_for_header() {
    let temp = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    assert_eq!(desired_viewport_height(&app, 107, 53), 53);
}

#[test]
fn overlay_viewport_uses_full_height_on_empty_transcript() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.bottom_pane.input = "/model".into();
    app.open_overlay(Overlay::CommandPalette);

    assert_eq!(desired_viewport_height(&app, 107, 53), 53);
}

#[test]
fn transcript_render_stays_above_bottom_pane() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.committed_turns.push(TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Show output".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "TRANSCRIPT_SENTINEL".into(),
                payload: None,
            },
        ],
    });
    app.bottom_pane.input = "composer text".into();

    let width = 80;
    let height = 14;
    let rendered = render_screen_text(&mut app, width, height);
    let lines = rendered.lines().collect::<Vec<_>>();
    let bottom_height = usize::from(desired_bottom_pane_height(&app, width, height));
    let transcript_end = usize::from(height).saturating_sub(bottom_height);
    let transcript = lines[..transcript_end].join("\n");
    let bottom = lines[transcript_end..].join("\n");

    assert!(transcript.contains("TRANSCRIPT_SENTINEL"));
    assert!(!bottom.contains("TRANSCRIPT_SENTINEL"));
    assert!(bottom.contains("composer text"));
}

#[test]
fn bottom_pane_background_covers_hint_and_footer_rows() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.bottom_pane.notice = Some("Prompt finished.".into());
    app.repo_slug = Some("hawkingrei/rara".into());
    app.snapshot.branch = "main".into();

    let width = 100;
    let height = 14;
    let buffer = render_screen_buffer(&mut app, width, height);
    let bottom_height = desired_bottom_pane_height(&app, width, height);
    let bottom_start = height.saturating_sub(bottom_height);
    let expected_bg = Color::Reset;

    for y in bottom_start..height {
        for x in 0..width {
            assert_eq!(
                buffer[(x, y)].bg,
                expected_bg,
                "bottom pane background missing at ({x}, {y})"
            );
        }
    }
}

#[test]
fn tool_summary_includes_apply_patch_target_files() {
    let entries = [TranscriptEntry {
        role: "Tool".into(),
        message: "apply_patch src/tui/render.rs, src/tui/runtime/events.rs".into(),
        payload: None,
    }];
    let refs = entries.iter().collect::<Vec<_>>();

    let rendered = current_turn_tool_summary(&refs, false, None).expect("tool summary");
    assert!(rendered.contains("Apply patch src/tui/render.rs, src/tui/runtime/events.rs"));
}

#[test]
fn tool_summary_includes_bash_result_status_and_output_tail() {
    let entries = [TranscriptEntry { role: "Tool".into(), message: "bash cd /Users/vl/Code/rara && cargo build 2>&1".into(), payload: None },
        TranscriptEntry { role: "Tool Result".into(), message: "bash failed with exit code 101\nstdout:\n   Compiling rara v0.1.0\nstderr:\nerror[E0425]: cannot find value `foo` in this scope".into(), payload: None }];
    let refs = entries.iter().collect::<Vec<_>>();

    let rendered = current_turn_tool_summary(&refs, false, None).expect("tool summary");
    assert!(rendered.contains("Run cd /Users/vl/Code/rara && cargo build 2>&1"));
    assert!(rendered.contains("bash failed with exit code 101"));
    assert!(rendered.contains("stdout:"));
    assert!(rendered.contains("Compiling rara v0.1.0"));
    assert!(rendered.contains("error[E0425]"));
}

#[test]
fn tool_summary_compacts_spawn_agent_instruction_json() {
    let entries = [TranscriptEntry {
        role: "Tool".into(),
        message: format!(
            "spawn_agent {}",
            json!({
                "name": "fix-assembler",
                "instruction": "Fix the file src/context/assembler.rs by removing the orphaned code block between the two cfg(test) markers. Read in small chunks and avoid one giant replacement payload."
            })
        ),
        payload: None,
    }];
    let refs = entries.iter().collect::<Vec<_>>();

    let rendered = current_turn_tool_summary(&refs, false, None).expect("tool summary");
    assert!(rendered.contains("Delegate fix-assembler: Fix the file src/context/assembler.rs"));
    assert!(rendered.contains('…'));
    assert!(!rendered.contains("\"instruction\""));
    assert!(!rendered.contains("avoid one giant replacement payload"));
}

#[test]
fn tool_action_label_uses_explore_icon_for_explore_agent() {
    let rendered = tool_action_label("explore_agent inspect the runtime path");
    assert!(rendered.is_some());
    assert!(rendered.unwrap().starts_with("🔍 Explore"));
}

#[test]
fn tool_action_label_uses_plan_icon_for_plan_agent() {
    let rendered = tool_action_label("plan_agent reorganize the module");
    assert!(rendered.is_some());
    assert!(rendered.unwrap().starts_with("📋 Plan"));
}

#[test]
fn tool_action_label_uses_team_icon_for_team_create() {
    let rendered = tool_action_label("team_create review PR");
    assert!(rendered.is_some());
    assert!(rendered.unwrap().starts_with("👥 Team"));
}

#[test]
fn renderable_transcript_lines_include_committed_and_active_turns() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.committed_turns.push(TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Earlier prompt".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "Committed answer".into(),
                payload: None,
            },
        ],
    });
    app.active_turn.entries.push(TranscriptEntry {
        role: "You".into(),
        message: "Current prompt".into(),
        payload: None,
    });

    let rendered = renderable_transcript_lines(&app, 100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("You"));
    assert!(rendered.contains("Earlier prompt"));
    assert!(rendered.contains("Committed answer"));
    assert!(rendered.contains("Current prompt"));
}

#[test]
fn renderable_transcript_lines_insert_turn_dividers_between_rounds() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.committed_turns = vec![
        TranscriptTurn {
            entries: vec![TranscriptEntry {
                role: "You".into(),
                message: "First prompt".into(),
                payload: None,
            }],
        },
        TranscriptTurn {
            entries: vec![TranscriptEntry {
                role: "Agent".into(),
                message: "Second reply".into(),
                payload: None,
            }],
        },
    ];
    app.active_turn.entries.push(TranscriptEntry {
        role: "You".into(),
        message: "Current prompt".into(),
        payload: None,
    });

    let rendered = renderable_transcript_lines(&app, 24)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    let divider = format!(" {}", "─".repeat(22));
    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.as_str() == divider.as_str())
            .count(),
        2
    );
}

#[test]
fn startup_header_renders_but_does_not_enter_transcript_lines() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let rendered = render_screen_text(&mut app, 100, 24);
    assert!(rendered.contains("── RARA"));

    let transcript = renderable_transcript_lines(&app, 100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!transcript.contains("── RARA"));
    assert!(!transcript.contains("directory:"));
}

#[test]
fn transcript_scroll_offset_keeps_zero_sticky_to_bottom() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.transcript_scroll = 0;

    assert_eq!(transcript_scroll_offset(&app, 3, 10), 7);

    app.scroll_transcript(-2);
    assert_eq!(transcript_scroll_offset(&app, 3, 10), 5);
}

#[test]
fn transcript_scroll_offset_uses_wrapped_visual_height() {
    let temp = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let lines = vec![
        Line::from("Agent"),
        Line::from("  This is a long streamed response that should wrap across rows."),
    ];

    let visual_rows = transcript_visual_row_count(&lines, 12);
    assert!(visual_rows > lines.len());
    assert_eq!(
        transcript_scroll_offset(&app, 3, visual_rows),
        visual_rows as u16 - 3
    );
}

#[test]
fn effective_height_includes_final_row_at_bottom_sticky() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    // Pre-build committed turns so that visual rows exceed a 5-row viewport.
    let entries: Vec<TranscriptEntry> = (0..8)
        .map(|i| TranscriptEntry {
            role: "Agent".into(),
            message: format!("Line {i}"),
            payload: None,
        })
        .collect();
    app.restore_committed_turns(vec![TranscriptTurn { entries }]);

    let viewport = transcript_viewport(&app, 80, 5);
    let (visible_lines, _inner) = viewport.visible_window(80, 5);

    // Effective height = 5 - 1 = 4. With scroll=0 (bottom sticky),
    // the viewport should show the last 4 content rows.
    assert_eq!(visible_lines.len(), 4);

    let last_line = visible_lines
        .last()
        .map(|line| line.to_string())
        .unwrap_or_default();
    assert!(
        last_line.contains("Line 7"),
        "bottom sticky should include final content row ('Line 7') not: {last_line}"
    );
}

#[test]
fn renderable_transcript_lines_cache_is_invalidated_when_committed_turns_change() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.restore_committed_turns(vec![TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "Agent".into(),
            message: "First answer".into(),
            payload: None,
        }],
    }]);

    let first = renderable_transcript_lines(&app, 100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(first.contains("First answer"));

    app.restore_committed_turns(vec![TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "Agent".into(),
            message: "Second answer".into(),
            payload: None,
        }],
    }]);

    let second = renderable_transcript_lines(&app, 100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!second.contains("First answer"));
    assert!(second.contains("Second answer"));
}

#[test]
fn transcript_viewport_is_independent_from_overlay_state() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.committed_turns.push(TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Earlier prompt".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "Committed answer".into(),
                payload: None,
            },
        ],
    });
    app.active_turn.entries.push(TranscriptEntry {
        role: "You".into(),
        message: "Current prompt".into(),
        payload: None,
    });

    let base = transcript_viewport(&app, 80, 18);
    app.overlay = Some(Overlay::Status(StatusTab::Overview));
    let with_overlay = transcript_viewport(&app, 80, 18);

    let base_rendered = base
        .lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let overlay_rendered = with_overlay
        .lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(base_rendered, overlay_rendered);
    assert_eq!(base.scroll_offset, with_overlay.scroll_offset);
}

#[test]
fn transcript_viewport_keeps_manual_scroll_when_overlay_opens() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.committed_turns.push(TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Earlier prompt".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: (1..=8)
                    .map(|idx| format!("Line {idx}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                payload: None,
            },
        ],
    });
    app.scroll_transcript(-3);

    let base = transcript_viewport(&app, 60, 8);
    app.overlay = Some(Overlay::Status(StatusTab::Overview));
    let with_overlay = transcript_viewport(&app, 60, 8);

    assert_eq!(base.scroll_offset, with_overlay.scroll_offset);
    assert_eq!(app.transcript_scroll, 3);
}

#[test]
fn command_palette_does_not_change_scrolled_viewport_height() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.transcript_scroll = 5;

    let base = desired_viewport_height(&app, 80, 24);
    app.overlay = Some(Overlay::CommandPalette);
    let with_palette = desired_viewport_height(&app, 80, 24);

    assert_eq!(base, 24);
    assert_eq!(base, with_palette);
}

#[test]
fn bottom_pane_grows_for_multiline_input() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let base = desired_bottom_pane_height(&app, 80, 24);
    app.bottom_pane.input = "first line\nsecond line\nthird line\nfourth line".into();
    let expanded = desired_bottom_pane_height(&app, 80, 24);

    assert_eq!(base, 5);
    assert!(expanded > base);
}

#[test]
fn bottom_pane_preserves_space_only_input_layout() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    app.bottom_pane.input = " ".into();
    let space_only = desired_bottom_pane_height(&app, 80, 24);

    app.bottom_pane.input = "  \n ".into();
    let multiline_space_only = desired_bottom_pane_height(&app, 80, 24);

    assert_eq!(space_only, 5);
    assert!(multiline_space_only >= space_only);
}

#[test]
fn bottom_pane_height_does_not_panic_on_tiny_terminal() {
    let temp = tempdir().expect("tempdir");
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    assert_eq!(desired_bottom_pane_height(&app, 80, 1), 1);
    assert_eq!(desired_bottom_pane_height(&app, 80, 3), 3);
}

#[test]
fn transcript_viewport_visible_window_keeps_partial_wrapped_line_offset() {
    let viewport = TranscriptViewport::new(
        vec![
            Line::from("• This is a long first line that wraps across rows."),
            Line::from("  Second line stays visible."),
        ],
        1,
        12,
    );

    let (lines, inner_scroll) = viewport.visible_window(12, 3);
    let rendered = lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(inner_scroll, 1);
    assert_eq!(rendered.len(), 1);
    assert!(rendered[0].contains("long first line"));
}

#[test]
fn transcript_viewport_visible_window_slices_to_visible_rows() {
    let viewport = TranscriptViewport::new(
        vec![
            Line::from("› First"),
            Line::from("• Second"),
            Line::from("  Third"),
            Line::from("  Fourth"),
        ],
        1,
        80,
    );

    // height=3 gives 3 visible content rows.
    let (lines, inner_scroll) = viewport.visible_window(80, 3);
    let rendered = lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(inner_scroll, 0);
    assert_eq!(rendered, vec!["• Second", "  Third", "  Fourth"]);
}

#[test]
fn exploration_summary_uses_codex_style_search_labels() {
    let entries = [
        TranscriptEntry {
            role: "Tool".into(),
            message: "list_files .".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Tool".into(),
            message: "glob src/**/*.rs".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Tool".into(),
            message: "grep planning mode src".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Tool".into(),
            message: "read_file src/main.rs".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Tool".into(),
            message: "bash rg --files src/tui".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Tool".into(),
            message: "bash cd src && rg -n \"render\" tui".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Agent".into(),
            message: "I will start by listing files and then inspect the main entrypoint.".into(),
            payload: None,
        },
    ];
    let refs = entries.iter().collect::<Vec<_>>();

    let rendered = current_turn_exploration_summary_from_entries(refs.as_slice(), false, None)
        .expect("exploration summary");
    assert!(rendered.contains("Find files src/tui"));
    assert!(rendered.contains("Search planning mode src"));
    assert!(rendered.contains("Read src/main.rs"));
    assert!(rendered.contains("Search render src/tui"));
    assert!(rendered.contains("more file(s) inspected"));
    assert!(!rendered.contains("Glob src/**/*.rs"));
    assert!(!rendered.contains("listing files"));
}

#[test]
fn rg_bash_search_is_not_duplicated_as_running_tool() {
    assert!(tool_action_label("bash rg --files src/tui").is_none());
    assert!(tool_action_label("bash cd src && rg -n \"render\" tui").is_none());
    assert_eq!(
        tool_action_label("bash cargo check"),
        Some("Run cargo check".to_string())
    );
}

#[test]
fn compact_progress_summary_lines_prioritizes_latest_note_and_recent_actions() {
    let actions = vec![
        "Read src/module_1.rs".to_string(),
        "Read src/module_2.rs".to_string(),
        "Read src/module_3.rs".to_string(),
    ];
    let notes = vec![
        "Initial inspection complete.".to_string(),
        "Next I will verify the persistence path.".to_string(),
    ];

    let rendered = compact_progress_summary_lines(
        actions.as_slice(),
        notes.as_slice(),
        2,
        "more exploration step(s)",
    );

    assert!(rendered.contains("Next I will verify the persistence path."));
    assert!(!rendered.contains("Initial inspection complete."));
    assert!(rendered.contains("... 1 more exploration step(s)"));
    assert!(rendered.contains("Read src/module_2.rs"));
    assert!(rendered.contains("Read src/module_3.rs"));
}

#[test]
fn compact_recent_first_summary_lines_puts_current_running_step_first() {
    let items = vec![
        "Run task 1".to_string(),
        "Run task 2".to_string(),
        "Run task 3".to_string(),
        "Run task 4".to_string(),
        "Run task 5".to_string(),
    ];

    let rendered = compact_recent_first_summary_lines(items.as_slice(), 4, "more running step(s)");

    let lines = rendered.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "└ Run task 5");
    assert_eq!(lines[1], "└ ... 1 more running step(s)");
    assert!(rendered.contains("Run task 4"));
    assert!(rendered.contains("Run task 2"));
    assert!(!rendered.contains("Run task 1"));
}

#[test]
fn exploration_summary_compacts_long_read_lists() {
    let entries = (1..=6)
        .map(|idx| TranscriptEntry {
            role: "Tool".into(),
            message: format!("read_file src/module_{idx}.rs"),
            payload: None,
        })
        .collect::<Vec<_>>();
    let refs = entries.iter().collect::<Vec<_>>();

    let rendered = current_turn_exploration_summary_from_entries(refs.as_slice(), false, None)
        .expect("exploration summary");
    assert!(rendered.contains("... 2 more file(s) inspected"));
    assert!(!rendered.contains("module_1.rs"));
    assert!(!rendered.contains("module_2.rs"));
    assert!(rendered.contains("module_3.rs"));
    assert!(rendered.contains("module_6.rs"));
}

#[test]
fn compact_summary_text_keeps_tail_of_long_explicit_blocks() {
    let summary = [
        "└ Read src/a.rs",
        "└ Read src/b.rs",
        "└ Read src/c.rs",
        "└ Read src/d.rs",
        "└ Read src/e.rs",
    ]
    .join("\n");

    let rendered = compact_summary_text(&summary, 4, "more exploration step(s)");
    assert!(rendered.contains("... 1 more exploration step(s)"));
    assert!(!rendered.contains("src/a.rs"));
    assert!(rendered.contains("src/b.rs"));
    assert!(rendered.contains("src/e.rs"));
}

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
        let mut guard = Self::remove(&keys);
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
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Provider));

    let rendered = render_screen_text(&mut app, 100, 24);
    let dir = display_directory_for_startup(&app);
    let rendered = rendered.replace(&dir, "<CWD>");
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
    assert!(!rendered.contains("Kimi (openai-compatible/kimi-k2.6)"));
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
    app.open_overlay(Overlay::ApiKeyEditor);

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
    app.open_overlay(Overlay::ApiKeyEditor);

    let rendered = render_screen_text(&mut app, 100, 24);
    assert!(rendered.contains("DeepSeek API Key"));
    assert!(rendered.contains("Paste a DeepSeek API key"));
    assert!(rendered.contains("Enter save and load models"));
    assert!(rendered.contains("Esc back to model picker"));
    assert!(!rendered.contains("Codex API Key"));
    assert!(!rendered.contains("Esc back to login guide"));
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

#[test]
fn resume_picker_snapshot() {
    use crate::thread_store::{CompactionRecord, ThreadMetadata, ThreadSummary};

    let temp = tempdir().expect("tempdir");
    let mut app_mut = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_760_000_000) as i64;

    let mut app = app_mut;
    app.recent_threads = vec![
        ThreadSummary {
            metadata: ThreadMetadata {
                session_id: "sess-abc123".into(),
                created_at: now - 86_400 * 4,
                updated_at: now - 86_400 * 4,
                history_len: 42,
                cwd: "/Users/dev/opensource/tidb".into(),
                branch: "master".into(),
                provider: "openai-compatible".into(),
                model: "deepseek-v4-pro".into(),
                agent_mode: "execute".into(),
                base_url: None,
                bash_approval: "auto".into(),
                origin_kind: "new".into(),
                forked_from_thread_id: None,
                transcript_len: 96,
            },
            preview: "https://github.com/pingcap/tidb/issues/67363 看一下这个issue".into(),
            compaction: CompactionRecord::default(),
        },
        ThreadSummary {
            metadata: ThreadMetadata {
                session_id: "sess-def456".into(),
                created_at: now - 86_400 * 5,
                updated_at: now - 86_400 * 5,
                history_len: 18,
                cwd: "/Users/dev/opensource/rara".into(),
                branch: "feat/resume".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4".into(),
                agent_mode: "planning".into(),
                base_url: None,
                bash_approval: "manual".into(),
                origin_kind: "new".into(),
                forked_from_thread_id: None,
                transcript_len: 64,
            },
            preview: "把resume picker改得像Codex".into(),
            compaction: CompactionRecord::default(),
        },
    ];

    app.overlay = Some(Overlay::ListPicker(ListPickerKind::Resume));
    app.resume_filter_cwd = false;
    app.cwd = "/Users/dev/opensource/rara".to_string();
    app.resume_picker_idx = 0;

    let rendered = render_screen_text(&mut app, 80, 20);
    assert_snapshot!("resume_picker", rendered);
}
