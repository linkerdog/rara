# Resume Picker Information

## Summary

The resume picker now uses a dedicated Codex-aligned picker surface instead of
only changing the row text inside the generic bottom list:

- the modal is expanded to a near-full-screen picker instead of a small bottom
  slice;
- sessions can be searched by preview, id, cwd, branch, provider, model, mode,
  or approval policy;
- the toolbar exposes cwd/all filtering and updated/created sorting;
- the list loads up to 200 recent sessions and scrolls the selected row;
- each row keeps the preview-first metadata hierarchy while using a dense
  two-line layout, with a third line only when compaction details exist.

## Background

Codex's resume picker is not just a richer row renderer. It is a dedicated
selection surface with filtering and sorting controls, so users can narrow a
large persisted session list before resuming. RARA already stored enough
metadata for that shape, but the previous `/resume` route used the generic
bottom list picker, loaded only a small fixed set, and had no visible search or
filter surface.

## Scope

This checkpoint keeps the existing `Overlay::ListPicker(ListPickerKind::Resume)`
route and adds the missing resume-specific behavior there. It also removes the
unused legacy `render_resume_picker_modal` path from `overlay_setup.rs` so there
is only one resume picker implementation to maintain. The row layout is kept
dense enough for repeated resume workflows: preview on the first line, runtime,
workspace, identifiers, and counts on the second line, and optional compaction
details on the third line.

The change does not add Codex's async transcript preview expansion, pagination,
or dense/comfortable view toggle.

## Validation

```bash
cargo test tui::list_picker::tests::resume_summary_lines_surface_runtime_location_and_compaction_metadata -- --nocapture
cargo test tui::list_picker::tests::resume_picker_key_event_treats_printable_keys_as_search_input -- --nocapture
cargo test tui::state::tests::resume_picker_refreshes_recent_threads_on_open -- --nocapture
cargo test tui::state::tests::resume_picker_loads_more_than_legacy_twenty_thread_cap -- --nocapture
cargo test tui::state::tests::resume_picker_search_filters_and_clear_restores_threads -- --nocapture
cargo fmt --check
cargo check
```

`cargo check` completed successfully with existing workspace warnings. The
focused tests above cover the row metadata contract, resume-specific key
handling, refresh-on-open behavior, the larger recent-session cap, and search
clearing.

## Follow-Ups

- Decide separately whether RARA should add transcript expansion, pagination, and
  density toggles to match the rest of Codex's picker behavior.
