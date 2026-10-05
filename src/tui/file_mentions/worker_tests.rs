use super::*;

fn request(root: &std::path::Path, generation: u64, query: &str) -> SearchRequest {
    SearchRequest {
        root: root.into(),
        generation,
        query: query.into(),
    }
}

fn completed(worker: &FileSearchWorker) -> SearchResponse {
    let (mut pending, deadline) = worker
        .shared
        .completed
        .wait_timeout_while(
            worker.shared.lock(),
            std::time::Duration::from_secs(10),
            |pending| pending.response.is_none(),
        )
        .unwrap();
    assert!(!deadline.timed_out(), "worker did not publish a result");
    pending.response.take().unwrap()
}

#[test]
fn worker_coalesces_queries_reuses_index_and_refreshes_after_close_or_root_change() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("first.rs"), "").unwrap();
    std::fs::create_dir(root.path().join("target")).unwrap();
    std::fs::write(root.path().join("target/ignored.rs"), "").unwrap();
    let mut worker = FileSearchWorker {
        shared: Arc::default(),
        thread: None,
    };
    worker.request(request(root.path(), 1, "missing"));
    worker.request(request(root.path(), 2, "rs"));
    let shared = worker.shared.clone();
    worker.thread = Some(std::thread::spawn(move || run(shared)));
    let result = completed(&worker);
    assert_eq!(result.generation, 2);
    assert_eq!(
        result.result.unwrap().results.matches[0].path,
        PathBuf::from("first.rs")
    );

    std::fs::write(root.path().join("second.rs"), "").unwrap();
    worker.request(request(root.path(), 3, "rs"));
    assert_eq!(completed(&worker).result.unwrap().results.matches.len(), 1);
    worker.clear();
    worker.request(request(root.path(), 4, "rs"));
    assert_eq!(completed(&worker).result.unwrap().results.matches.len(), 2);

    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("other.rs"), "").unwrap();
    worker.request(request(other.path(), 5, "rs"));
    let result = completed(&worker).result.unwrap().results;
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].root, other.path());
    assert_eq!(result.matches[0].path, PathBuf::from("other.rs"));
}

#[test]
fn query_replacement_cancels_ranking_and_root_or_close_cancels_discovery() {
    let worker = FileSearchWorker {
        shared: Arc::default(),
        thread: None,
    };
    worker.request(request(std::path::Path::new("one"), 1, "a"));
    let discovery = worker.shared.lock().discovery.clone();
    let ranking = worker.shared.lock().ranking.clone();
    worker.request(request(std::path::Path::new("one"), 2, "ab"));
    assert!(ranking.is_cancelled());
    assert!(!discovery.is_cancelled());
    worker.request(request(std::path::Path::new("two"), 3, "ab"));
    assert!(discovery.is_cancelled());
    let discovery = worker.shared.lock().discovery.clone();
    worker.clear();
    assert!(discovery.is_cancelled());
    assert!(worker.shared.lock().request.is_none());
}

#[test]
fn worker_surfaces_discovery_errors_and_recovers_on_the_next_request() {
    let root = tempfile::tempdir().unwrap();
    let worker = FileSearchWorker::start().unwrap();
    worker.request(request(&root.path().join("missing"), 1, ""));
    assert!(completed(&worker).result.is_err());
    std::fs::write(root.path().join("valid.rs"), "").unwrap();
    worker.request(request(root.path(), 2, ""));
    assert_eq!(completed(&worker).result.unwrap().results.matches.len(), 1);
}
