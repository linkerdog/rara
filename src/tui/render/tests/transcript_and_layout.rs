use super::*;

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

    let rendered = committed_turn_cell(entries.as_slice(), Some(Path::new(".")), false, None)
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
        thinking_duration: None,
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
        thinking_duration: None,
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
fn shell_approval_panel_keeps_actions_visible() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::Approval,
            title: "Shell Approval".into(),
            summary: "cargo check 2>&1 | head -80".into(),
            options: Vec::new(),
            note: None,
            approval: Some(PendingApprovalSnapshot {
                tool_use_id: "toolu_123".into(),
                command: "cargo check 2>&1 | head -80".into(),
                allow_net: false,
                payload: BashCommandInput {
                    command: Some("cargo check 2>&1 | head -80".into()),
                    cwd: Some("/home/hawkingrei/devel/opensource/rara".into()),
                    ..Default::default()
                },
            }),
            source: None,
            created_at_epoch_seconds: None,
        });

    let rendered = render_screen_text(&mut app, 80, 14);

    assert!(rendered.contains("# Permission Required"));
    assert!(rendered.contains("[1] Allow once"));
    assert!(rendered.contains("[2] Allow prefix"));
    assert!(rendered.contains("[3] Allow session"));
    assert!(rendered.contains("[4] Reject"));
}

#[test]
fn shell_approval_panel_uses_the_standard_bottom_pane_surface() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::Approval,
            title: "Shell Approval".into(),
            summary: "cargo check".into(),
            options: Vec::new(),
            note: None,
            approval: Some(PendingApprovalSnapshot {
                tool_use_id: "toolu_123".into(),
                command: "cargo check".into(),
                allow_net: false,
                payload: BashCommandInput::default(),
            }),
            source: None,
            created_at_epoch_seconds: None,
        });

    let width = 80;
    let height = 14;
    let buffer = render_screen_buffer(&mut app, width, height);
    let bottom_start = height.saturating_sub(desired_bottom_pane_height(&app, width, height));

    for y in bottom_start..height {
        for x in 0..width {
            assert_eq!(
                buffer[(x, y)].bg,
                Color::Reset,
                "approval dock must not replace the standard bottom-pane surface at ({x}, {y})"
            );
        }
    }
}

#[test]
fn interaction_panel_keeps_actions_visible_with_three_detail_lines() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::RequestInput,
            title: "line one\nline two\nline three".into(),
            summary: "answer planning question".into(),
            options: Vec::new(),
            note: None,
            approval: None,
            source: Some("plan_agent".into()),
            created_at_epoch_seconds: None,
        });

    let rendered = render_screen_text(&mut app, 80, 14);

    assert!(rendered.contains("Planning Question"));
    assert!(rendered.contains("[Enter] Continue Plan"));
    assert!(rendered.contains("[I] Start Implementation"));
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
fn tool_summary_uses_typed_tool_identity_before_role_strings() {
    let entries = [
        TranscriptEntry {
            role: "legacy-start".into(),
            message: "bash cargo check".into(),
            payload: Some(TranscriptEntryPayload::Tool(ToolTranscriptPayload {
                call_id: Some("tool-call-1".into()),
                name: "bash".into(),
                status: ToolTranscriptStatus::Running,
            })),
        },
        TranscriptEntry {
            role: "legacy-end".into(),
            message: "bash finished with exit code 0".into(),
            payload: Some(TranscriptEntryPayload::Tool(ToolTranscriptPayload {
                call_id: Some("tool-call-1".into()),
                name: "bash".into(),
                status: ToolTranscriptStatus::Completed,
            })),
        },
    ];

    let refs = entries.iter().collect::<Vec<_>>();
    let rendered = current_turn_tool_summary(&refs, false, None).expect("tool summary");

    assert!(rendered.contains("Run cargo check"));
    assert!(rendered.contains("bash finished with exit code 0"));
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
        thinking_duration: None,
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
            thinking_duration: None,
            entries: vec![TranscriptEntry {
                role: "You".into(),
                message: "First prompt".into(),
                payload: None,
            }],
        },
        TranscriptTurn {
            thinking_duration: None,
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
    app.restore_committed_turns(vec![TranscriptTurn {
        thinking_duration: None,
        entries,
    }]);

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
        thinking_duration: None,
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
        thinking_duration: None,
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
        thinking_duration: None,
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
        thinking_duration: None,
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
