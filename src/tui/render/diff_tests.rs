use super::render_patch_preview;
use crate::tui::text_wrap::display_width;

#[test]
fn renders_patch_preview_with_diff_signs() {
    let lines = render_patch_preview(
        "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n context\n*** End Patch",
        80,
    )
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");

    assert!(lines.contains("* Edited src/lib.rs (+1 -1)"));
    assert!(lines.contains("- old"));
    assert!(lines.contains("+ new"));
    assert!(lines.contains("  context"));
}

#[test]
fn renders_patch_preview_grouped_by_file() {
    let lines = render_patch_preview(
        "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** Add File: src/new.rs\n+hello\n*** End Patch",
        80,
    )
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");

    assert!(lines.contains("* Changed 2 files (+2 -1)"));
    assert!(lines.contains("src/lib.rs (+1 -1)"));
    assert!(lines.contains("src/new.rs (+1 -0)"));
}

#[test]
fn large_first_file_keeps_every_operation_before_hunks() {
    let mut patch = "*** Begin Patch\n*** Add File: first.rs\n".to_string();
    for index in 0..200 {
        patch.push_str(&format!("+body_{index:03}\n"));
    }
    patch.push_str("*** Delete File: deleted.rs\n*** Update File: old.rs\n*** Move to: moved.rs\n@@\n-old\n+new\n*** Add File: last.rs\n+last\n*** End Patch");
    let rendered = render_patch_preview(&patch, 80)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let body = rendered.find("+ body_000").unwrap();
    for header in [
        "Deleted deleted.rs",
        "Moved old.rs -> moved.rs",
        "Added last.rs",
    ] {
        assert!(
            rendered
                .find(header)
                .is_some_and(|position| position < body),
            "{header}: {rendered}"
        );
    }
    assert!(rendered.contains("120 more diff line(s)"));
    assert!(rendered.contains("+ last"));
}

#[test]
fn each_file_has_its_own_budget_and_composes_producer_omissions() {
    let mut patch =
        "*** Begin Patch\n*** Add File: first.rs\n*** Preview Stats: +200 -0\n".to_string();
    patch.push_str(&"+first\n".repeat(120));
    patch.push_str("*** Preview Omitted: 80\n*** Add File: exact.rs\n");
    patch.push_str(&"+exact\n".repeat(80));
    patch.push_str("*** Add File: last.rs\n");
    patch.push_str(&"+last\n".repeat(81));
    patch.push_str("*** End Patch");
    let rows = render_patch_preview(&patch, 80)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        rows.iter().filter(|row| row.contains("+ first")).count(),
        80
    );
    assert_eq!(
        rows.iter().filter(|row| row.contains("+ exact")).count(),
        80
    );
    assert_eq!(rows.iter().filter(|row| row.contains("+ last")).count(), 80);
    let rendered = rows.join("\n");
    assert!(rendered.contains("* Changed 3 files (+361 -0)"));
    assert!(rendered.contains("first.rs (+200 -0)"));
    assert!(rendered.contains("120 more diff line(s)"));
    assert!(rendered.contains("1 more diff line(s)"));
    assert_eq!(rendered.matches("more diff line(s)").count(), 2);
    assert!(!rendered.contains("*** Preview"));
}

#[test]
fn deletion_counts_distinguish_missing_content_from_known_empty_files() {
    for (metadata, expected) in [
        ("", "-?"),
        ("*** Preview Stats: +0 -0\n", "-0"),
        ("*** Preview Stats: +0 -42\n", "-42"),
    ] {
        let patch = format!("*** Begin Patch\n*** Delete File: gone.rs\n{metadata}*** End Patch");
        let rows = render_patch_preview(&patch, 80);
        assert_eq!(
            rows[0].to_string(),
            format!("* Deleted gone.rs (+0 {expected})")
        );
        assert!(
            rows.iter()
                .any(|row| row.to_string().contains("no inline diff preview"))
        );
    }
}

#[test]
fn narrow_previews_preserve_paths_clusters_counts_and_diff_styles() {
    use crate::tui::theme::{ThemeToken, theme_color};

    let source = "src/my long 路径 👩‍💻 file.rs";
    let target = "src/new 路径 👩‍💻 file.rs";
    let patch = format!(
        "*** Begin Patch\n*** Update File: {source}\n*** Move to: {target}\n@@\n-old\n+e\u{301} 👩‍💻 界\tend\n*** End Patch"
    );
    for width in [80, 40, 20, 10, 6] {
        let rows = render_patch_preview(&patch, width);
        for row in &rows {
            assert!(
                display_width(&row.to_string()) <= usize::from(width),
                "{width}: {row}"
            );
        }
        let joined = rows.iter().map(ToString::to_string).collect::<String>();
        assert!(
            joined.contains(&format!("Moved {source} -> {target} (+1 -1)")),
            "{width}: {joined}"
        );
        let content = rows
            .iter()
            .flat_map(|row| &row.spans)
            .filter(|span| {
                span.style.fg == Some(theme_color(ThemeToken::DiffAddFg))
                    && span.style.bg == Some(theme_color(ThemeToken::DiffAddBg))
                    && !span
                        .style
                        .add_modifier
                        .contains(ratatui::style::Modifier::BOLD)
            })
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(content, "e\u{301} 👩‍💻 界    end", "{width}");
        assert!(
            rows.iter()
                .flat_map(|row| &row.spans)
                .any(|span| span.content == "+"
                    && span.style.fg == Some(theme_color(ThemeToken::DiffAddFg)))
        );
    }
    assert!(render_patch_preview(&patch, 0).is_empty());
    for width in [1, 2, 3] {
        assert!(
            render_patch_preview(&patch, width)
                .iter()
                .all(|row| display_width(&row.to_string()) <= usize::from(width))
        );
    }
}

#[test]
fn message_margin_preserves_context_whitespace_and_metadata_like_code() {
    let message = "apply_patch validated\ndiff:\n  *** Begin Patch\n  *** Update File: code.txt\n  @@\n     indented\n   *** Preview Stats: +999 -999\n  -old\n  +new\n  *** End Patch";
    let rows = super::render_message_diff_preview(Some("Tool Result"), message, 80).unwrap();
    let rendered = rows
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("code.txt (+1 -1)"));
    assert!(rendered.contains("        indented"));
    assert!(rendered.contains("*** Preview Stats: +999 -999"));
}

#[test]
fn multi_file_inventory_layout() {
    let patch = "*** Begin Patch\n*** Update File: src/old.rs\n*** Move to: src/new.rs\n@@\n-before\n+after\n*** Delete File: obsolete.rs\n*** Preview Stats: +0 -3\n*** Add File: empty.rs\n*** Preview Stats: +0 -0\n*** End Patch";
    let rendered = render_patch_preview(patch, 50)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!("multi_file_inventory", rendered);
}
