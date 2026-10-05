use super::*;

#[test]
fn action_preview_keeps_later_files_and_full_counts() {
    let mut patch = "*** Begin Patch\n*** Add File: first.rs\n".to_string();
    for index in 0..200 {
        patch.push_str(&format!("+body_{index:03}\n"));
    }
    patch.push_str("*** Delete File: deleted.rs\n*** Update File: old.rs\n*** Move to: moved.rs\n@@\n-old\n+new\n*** Add File: last.rs\n+last\n*** End Patch");
    let action = build_patch_action(&patch, |path| {
        Ok(match path {
            "deleted.rs" => Some("one\ntwo\nthree\n".into()),
            "old.rs" => Some("old\n".into()),
            _ => None,
        })
    })
    .unwrap();
    assert!(action.preview.truncated);
    for directive in [
        "*** Delete File: deleted.rs",
        "*** Move to: moved.rs",
        "*** Add File: last.rs",
    ] {
        assert!(action.preview.text.contains(directive), "{directive}");
    }
    assert!(action.preview.text.contains("*** Preview Stats: +200 -0"));
    assert!(action.preview.text.contains("*** Preview Stats: +0 -3"));
    assert!(action.preview.text.contains("*** Preview Omitted: 80"));
}

#[test]
fn raw_preview_budgets_each_file_and_retains_move_directives() {
    let mut patch = "*** Begin Patch\n*** Add File: exact.rs\n".to_string();
    patch.push_str(&"+exact\n".repeat(120));
    patch.push_str("*** Update File: old.rs\n*** Move to: new.rs\n@@\n");
    patch.push_str(&"-old\n".repeat(60));
    patch.push_str(&"+new\n".repeat(60));
    patch.push_str("*** End of File\n*** Delete File: gone.rs\n*** End Patch");
    let (preview, truncated) = patch_preview(&patch);
    assert!(truncated);
    assert_eq!(preview.matches("+exact\n").count(), 120);
    assert!(preview.contains("*** Preview Stats: +120 -0"));
    assert!(preview.contains("*** Preview Stats: +60 -60"));
    assert!(preview.contains("*** Move to: new.rs"));
    assert!(preview.contains("*** Preview Omitted: 1\n*** End of File"));
    assert_eq!(preview.matches("*** Preview Omitted:").count(), 1);
    assert!(preview.ends_with("*** Delete File: gone.rs\n*** End Patch"));
}

#[test]
fn exact_budget_and_empty_deletion_have_no_truncation() {
    let mut patch = "*** Begin Patch\n*** Add File: exact.rs\n".to_string();
    patch.push_str(&"+exact\n".repeat(120));
    patch.push_str("*** Delete File: empty.rs\n*** End Patch");
    let action = build_patch_action(&patch, |_| Ok(Some(String::new()))).unwrap();
    assert!(!action.preview.truncated);
    assert!(action.preview.text.contains("*** Preview Stats: +0 -0"));
    assert!(!action.preview.text.contains("*** Preview Omitted:"));
}
