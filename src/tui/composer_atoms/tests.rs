use super::*;

#[test]
fn file_encoding_recovers_atomic_ranges_without_file_reads() {
    for path in [
        "src/parser.rs",
        "src/a b.rs",
        "src/a\"b\\c.rs",
        "src/\u{754c}.rs",
    ] {
        let token = encode_mention(path);
        assert_eq!(serde_json::from_str::<String>(&token[1..]).unwrap(), path);
        let text = format!("inspect {token} now");
        assert_eq!(
            atoms(&text, &[]),
            [ComposerAtom {
                range: 8..8 + token.chars().count(),
                kind: ComposerAtomKind::FileMention
            }]
        );
    }
    assert!(atoms("email@\"address\" @\"unfinished", &[]).is_empty());
}

#[test]
fn edits_cross_whole_mentions_and_rebase_owned_pastes() {
    let mut pane = BottomPaneModel::new();
    let token = encode_mention("a b.rs");
    pane.input = format!("x {token} [paste] tail");
    let start = 3 + token.chars().count();
    pane.large_paste_pending.push(OwnedPaste {
        range: start..start + 7,
        label: "[paste]".into(),
        content: "expanded".into(),
    });
    pane.edit_composer(3..4, "");
    assert_eq!(pane.input, "x  [paste] tail");
    assert_eq!(pane.large_paste_pending[0].range, 3..10);
    pane.edit_composer(0..0, "\u{754c} ");
    assert_eq!(pane.large_paste_pending[0].range, 5..12);
    pane.expand_owned_pastes();
    assert_eq!(pane.input, "\u{754c} x  expanded tail");
}

#[test]
fn paste_expansion_is_owned_and_never_recursive() {
    let mut pane = BottomPaneModel::new();
    pane.input = "[one] [two] [one]".into();
    pane.large_paste_pending = vec![
        OwnedPaste {
            range: 0..5,
            label: "[one]".into(),
            content: "literal [two]".into(),
        },
        OwnedPaste {
            range: 6..11,
            label: "[two]".into(),
            content: "payload".into(),
        },
    ];
    pane.expand_owned_pastes();
    assert_eq!(pane.input, "literal [two] payload [one]");
    assert!(pane.large_paste_pending.is_empty());
}

#[test]
fn deleting_any_part_of_a_paste_releases_its_hidden_payload() {
    let mut pane = BottomPaneModel::new();
    pane.input = "[paste]".into();
    pane.large_paste_pending.push(OwnedPaste {
        range: 0..7,
        label: "[paste]".into(),
        content: "hidden payload".into(),
    });
    pane.edit_composer(2..3, "");
    assert!(pane.input.is_empty());
    assert!(pane.large_paste_pending.is_empty());
    pane.input = "[paste]".into();
    pane.expand_owned_pastes();
    assert_eq!(pane.input, "[paste]");
}

#[test]
fn atomic_layout_preserves_source_boundaries_at_every_viewport_width() {
    use crate::tui::composer_text::{VisualPosition, WrapConfig, wrapped_composer};
    let mut pane = BottomPaneModel::new();
    pane.input = format!(
        "before {}\u{301} after\nend",
        encode_mention("src/\u{202e}\u{754c} long.rs")
    );
    let atom = pane.composer_atoms().remove(0).range;
    for width in 1..80 {
        let layout = wrapped_composer(&pane, WrapConfig::composer(width));
        for row in 0..layout.rows().len() + 1 {
            for column in 0..width as usize {
                let offset = layout.offset_for_position(VisualPosition { row, column });
                assert!(
                    !(atom.start < offset && offset < atom.end),
                    "width {width}, offset {offset}"
                );
                let cursor = layout.cursor_position(offset);
                assert!(cursor.column < width as usize);
            }
        }
        assert_eq!(pane.floor_atom_boundary(atom.end - 1), atom.start);
        assert_eq!(pane.ceil_atom_boundary(atom.start + 1), atom.end);
    }
}

#[test]
fn mentions_wrap_whole_and_paste_ownership_invalidates_the_wrap_cache() {
    use crate::tui::composer_text::{WrapConfig, wrapped_composer};
    let mut pane = BottomPaneModel::new();
    pane.input = "a @\"b.rs\" z".into();
    assert_eq!(
        wrapped_composer(&pane, WrapConfig::composer(10)).rows(),
        &["› a ", "  @\"b.rs\" ", "  z"]
    );
    assert_eq!(
        wrapped_composer(&pane, WrapConfig::composer(7)).rows(),
        &["› a ", "  @\"b.…", "   z"]
    );
    pane.input = "[long paste label]".into();
    let plain = wrapped_composer(&pane, WrapConfig::composer(8));
    pane.large_paste_pending.push(OwnedPaste {
        range: 0..18,
        label: pane.input.clone(),
        content: "payload".into(),
    });
    let atomic = wrapped_composer(&pane, WrapConfig::composer(8));
    assert!(!std::sync::Arc::ptr_eq(&plain, &atomic));
    assert_eq!(atomic.rows(), &["› [long…"]);
    pane.expand_owned_pastes();
    assert_eq!(pane.input, "payload");
}
