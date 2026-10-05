use rara_tools::{patch::ApplyPatchTool, tool::Tool};

use super::*;
use crate::tui::message_role::MessageRole;
use crate::tui::state::TranscriptEntry;
use crate::tui::text_wrap::display_width;

#[tokio::test]
async fn native_patch_results_preserve_every_file_through_transcript_rendering() {
    for dry_run in [true, false] {
        let dir = tempdir().unwrap();
        let first = dir.path().join("first.rs");
        let edit = dir.path().join("edit.rs");
        let deleted = dir.path().join("deleted.rs");
        let old = dir.path().join("old.rs");
        let moved = dir.path().join("moved.rs");
        let last = dir.path().join("last.rs");
        std::fs::write(&edit, "old\n").unwrap();
        std::fs::write(&deleted, "one\ntwo\nthree\n").unwrap();
        std::fs::write(&old, "old\n").unwrap();
        let mut patch = format!("*** Begin Patch\n*** Add File: {}\n", first.display());
        for index in 0..200 {
            patch.push_str(&format!("+body_{index:03}\n"));
        }
        patch.push_str(&format!(
            "*** Update File: {}\n@@\n-old\n+new\n*** Delete File: {}\n*** Update File: {}\n*** Move to: {}\n@@\n-old\n+new\n*** Add File: {}\n+last\n*** End Patch",
            edit.display(), deleted.display(), old.display(), moved.display(), last.display(),
        ));
        let result = ApplyPatchTool::default()
            .call(json!({"patch": patch, "dry_run": dry_run}))
            .await
            .unwrap();
        assert_eq!(result["diff_truncated"], true);
        assert_eq!(result["line_delta"], json!({"added": 203, "removed": 5}));
        assert_eq!(deleted.exists(), dry_run);
        assert_eq!(moved.exists(), !dry_run);
        let message = format_tool_result("apply_patch", &result.to_string());
        let entries = [TranscriptEntry::new(MessageRole::ToolResult, message)];
        for width in [80, 40, 20] {
            let rows = crate::tui::render::committed_turn_lines(&entries, None, width, false, None);
            assert!(
                rows.iter()
                    .all(|row| display_width(&row.to_string()) <= usize::from(width))
            );
            let rendered = rows.iter().map(ToString::to_string).collect::<String>();
            assert!(rendered.contains("Changed 5 files (+203 -5)"));
            let first_hunk = rendered.find("+ body_000").unwrap();
            for expected in [
                format!("Added {} (+200 -0)", first.display()),
                format!("Edited {} (+1 -1)", edit.display()),
                format!("Deleted {} (+0 -3)", deleted.display()),
                format!("Moved {} -> {} (+1 -1)", old.display(), moved.display()),
                format!("Added {} (+1 -0)", last.display()),
            ] {
                assert!(
                    rendered
                        .find(&expected)
                        .is_some_and(|position| position < first_hunk),
                    "{width}: {expected}\n{rendered}"
                );
            }
            assert!(rendered.contains("120 more diff line(s)"));
            assert!(rendered.contains("+ last"));
            assert!(!rendered.contains("*** Preview"));
        }
    }
}
