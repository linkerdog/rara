use super::*;

fn limits(files: usize, bytes: usize) -> IndexLimits {
    IndexLimits {
        max_files: NonZeroUsize::new(files).unwrap(),
        max_path_bytes: NonZeroUsize::new(bytes).unwrap(),
    }
}

#[test]
fn index_preserves_ignore_policy_and_rank_order_without_rewalking() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join(".gitignore"), "ignored.rs\n").unwrap();
    for path in [
        "src/main.rs",
        "src/parser.rs",
        "src/pars\u{00e9}.rs",
        "ignored.rs",
    ] {
        std::fs::write(root.path().join(path), "").unwrap();
    }
    let options = FileSearchOptions {
        follow_links: false,
        ..Default::default()
    };
    let cancellation = SearchCancellation::default();
    let index = FileSearchIndex::build(
        root.path().into(),
        &options,
        limits(100, 10000),
        &cancellation,
    )
    .unwrap()
    .unwrap();
    let expected = crate::search_files("par", vec![root.path().into()], options).unwrap();
    std::fs::remove_dir_all(root.path().join("src")).unwrap();
    let found = index
        .search("par", NonZeroUsize::new(50).unwrap(), &cancellation)
        .unwrap();
    assert_eq!(found.results.matches, expected.matches);
    assert_eq!(found.results.total_match_count, 2);
    assert!(!found.index_truncated);
    assert!(
        index
            .search("ignored", NonZeroUsize::new(50).unwrap(), &cancellation)
            .unwrap()
            .results
            .matches
            .is_empty()
    );
}

#[test]
fn index_limits_and_top_k_are_reported_separately() {
    let root = tempfile::tempdir().unwrap();
    for path in ["a.rs", "b.rs", "c.rs"] {
        std::fs::write(root.path().join(path), "").unwrap();
    }
    let cancellation = SearchCancellation::default();
    for budget in [limits(2, 1000), limits(100, 8)] {
        let index = FileSearchIndex::build(
            root.path().into(),
            &FileSearchOptions::default(),
            budget,
            &cancellation,
        )
        .unwrap()
        .unwrap();
        let result = index
            .search("", NonZeroUsize::new(1).unwrap(), &cancellation)
            .unwrap();
        assert!(result.index_truncated);
        assert_eq!(result.results.scanned_entry_count, 2);
        assert_eq!(result.results.matches.len(), 1);
        assert!(result.results.truncated);
    }
}

#[test]
fn cancellation_discards_incomplete_scoring_and_prevents_discovery() {
    let cancellation = SearchCancellation::default();
    cancellation.cancel();
    assert!(
        FileSearchIndex::build(
            PathBuf::from("/missing-search-root"),
            &FileSearchOptions::default(),
            limits(100, 1000),
            &cancellation
        )
        .unwrap()
        .is_none()
    );
    let index = FileSearchIndex {
        root: PathBuf::new(),
        paths: vec!["a.rs".into(), "b.rs".into(), "c.rs".into()],
        truncated: false,
        skipped_non_utf8: 0,
    };
    let mut checked = 0;
    assert!(
        index
            .search_until("rs", NonZeroUsize::new(2).unwrap(), || {
                checked += 1;
                checked > 1
            })
            .is_none()
    );
    assert_eq!(checked, 2);
}

#[test]
fn bounded_heap_matches_full_search_for_ties_and_unicode_scores() {
    let root = tempfile::tempdir().unwrap();
    for path in ["a.rs", "parser.rs", "parse.rs", "b.rs", "\u{754c}parser.rs"] {
        std::fs::write(root.path().join(path), "").unwrap();
    }
    let options = FileSearchOptions {
        limit: NonZeroUsize::new(2).unwrap(),
        ..Default::default()
    };
    let cancellation = SearchCancellation::default();
    let index = FileSearchIndex::build(
        root.path().into(),
        &options,
        limits(100, 10000),
        &cancellation,
    )
    .unwrap()
    .unwrap();
    for query in ["", "rs", "par", "\u{754c}"] {
        let expected =
            crate::search_files(query, vec![root.path().into()], options.clone()).unwrap();
        let found = index.search(query, options.limit, &cancellation).unwrap();
        assert_eq!(found.results.matches, expected.matches, "{query}");
        assert_eq!(found.results.total_match_count, expected.total_match_count);
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_are_reported_without_lossy_file_references() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    let name = std::ffi::OsString::from_vec(vec![b'a', 0xff]);
    std::fs::write(root.path().join(name), "").unwrap();
    std::fs::write(root.path().join("valid.rs"), "").unwrap();
    let cancellation = SearchCancellation::default();
    let index = FileSearchIndex::build(
        root.path().into(),
        &FileSearchOptions::default(),
        limits(100, 10000),
        &cancellation,
    )
    .unwrap()
    .unwrap();
    let result = index
        .search("", NonZeroUsize::new(50).unwrap(), &cancellation)
        .unwrap();
    assert_eq!(result.skipped_non_utf8, 1);
    assert_eq!(result.results.matches[0].path, PathBuf::from("valid.rs"));
    assert_eq!(result.results.matches.len(), 1);
}
