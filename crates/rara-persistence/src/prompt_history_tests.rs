use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::*;

fn entry(text: &str) -> PromptHistoryEntry {
    PromptHistoryEntry::new(text).expect("retained prompt")
}

#[test]
fn filters_before_trimming_and_redacts_before_retaining() {
    for input in [
        " private prompt",
        "",
        "\n\t",
        &"x".repeat(MAX_PROMPT_BYTES + 1),
    ] {
        assert!(PromptHistoryEntry::new(input).is_none());
    }
    let value = entry("inspect token=0123456789abcdef then continue");
    assert_eq!(
        value.text(),
        "inspect token=[REDACTED_SECRET] then continue"
    );
    assert!(
        !serde_json::to_string(&value)
            .unwrap()
            .contains("0123456789abcdef")
    );
}

#[test]
fn missing_read_creates_nothing_and_reopening_recalls_once() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let store = PromptHistoryStore::new(&home);
    assert!(store.load().unwrap().entries.is_empty());
    assert!(!home.exists());
    let prompt = entry("first\nsecond line");
    store.append(&prompt).unwrap();
    store.append(&prompt).unwrap();
    assert_eq!(
        PromptHistoryStore::new(&home).load().unwrap().entries,
        vec![prompt]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn readers_recover_complete_records_without_mutating_a_torn_log() {
    let dir = tempfile::tempdir().unwrap();
    let store = PromptHistoryStore::new(dir.path());
    let first = entry("first");
    let last = entry("last");
    let mut bytes = serde_json::to_vec(&first).unwrap();
    bytes.extend_from_slice(b"\nnot-json secret=0123456789abcdef\n");
    bytes.extend(serde_json::to_vec(&last).unwrap());
    bytes.extend_from_slice(b"\n{\"unfinished\":");
    fs::write(store.path(), &bytes).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.entries, vec![first.clone(), last.clone()]);
    assert_eq!(loaded.skipped_lines, 1);
    assert_eq!(fs::read(store.path()).unwrap(), bytes);
    let next = entry("next");
    store.append(&next).unwrap();
    assert_eq!(store.load().unwrap().entries, vec![first, last, next]);
    assert!(
        !fs::read_to_string(store.path())
            .unwrap()
            .contains("0123456789abcdef")
    );
}

#[test]
fn compaction_preserves_newest_records_with_count_and_byte_limits() {
    let dir = tempfile::tempdir().unwrap();
    let store = PromptHistoryStore::new(dir.path());
    for index in 0..MAX_HISTORY_ENTRIES + 8 {
        store.append(&entry(&format!("prompt {index}"))).unwrap();
    }
    let loaded = store.load().unwrap();
    assert_eq!(loaded.entries.len(), MAX_HISTORY_ENTRIES);
    assert_eq!(loaded.entries[0].text(), "prompt 8");
    for index in 0..80 {
        store
            .append(&entry(&format!(
                "{index}:{}",
                "x".repeat(MAX_PROMPT_BYTES - 5)
            )))
            .unwrap();
        assert!(fs::metadata(store.path()).unwrap().len() <= MAX_HISTORY_BYTES as u64);
    }
    let loaded = store.load().unwrap();
    assert!(loaded.entries.len() < MAX_HISTORY_ENTRIES);
    assert!(loaded.entries.last().unwrap().text().starts_with("79:"));
}

#[test]
fn noncanonical_whitespace_counts_toward_the_file_cap() {
    let dir = tempfile::tempdir().unwrap();
    let store = PromptHistoryStore::new(dir.path());
    let prompt = entry("older");
    let mut bytes = serde_json::to_vec(&prompt).unwrap();
    bytes.resize(MAX_HISTORY_BYTES - 1, b' ');
    bytes.push(b'\n');
    fs::write(store.path(), bytes).unwrap();
    let next = entry("newer");
    store.append(&next).unwrap();
    assert_eq!(store.load().unwrap().entries, vec![prompt, next]);
    assert!(fs::metadata(store.path()).unwrap().len() <= MAX_HISTORY_BYTES as u64);
}

#[test]
fn read_window_is_bounded_and_loaded_secrets_are_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let store = PromptHistoryStore::new(dir.path());
    let mut file = File::create(store.path()).unwrap();
    file.write_all(&vec![b'x'; MAX_HISTORY_BYTES + 10]).unwrap();
    let record = serde_json::json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "text": "password=0123456789abcdef"
    });
    writeln!(file, "\n{record}").unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.entries.len(), 1);
    assert_eq!(loaded.entries[0].text(), "password=[REDACTED_SECRET]");
    store.append(&entry("next")).unwrap();
    assert!(
        !fs::read_to_string(store.path())
            .unwrap()
            .contains("0123456789abcdef")
    );
}

#[test]
fn lock_contention_is_bounded_but_readers_do_not_wait() {
    let dir = tempfile::tempdir().unwrap();
    let store = PromptHistoryStore::new(dir.path());
    let prompt = entry("existing");
    store.append(&prompt).unwrap();
    let lock = acquire_lock(&dir.path().join("prompt_history.lock")).unwrap();
    assert_eq!(store.load().unwrap().entries, vec![prompt]);
    let error = store.append(&entry("blocked")).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::TimedOut
    );
    drop(lock);
    store.append(&entry("after lock release")).unwrap();
}

#[test]
fn separate_processes_share_one_lock_across_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let store = PromptHistoryStore::new(dir.path());
    for index in 0..180 {
        store.append(&entry(&format!("seed {index}"))).unwrap();
    }
    let mut children = (0..4)
        .map(|worker| {
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "prompt_history::tests::process_writer",
                    "--ignored",
                ])
                .env("RARA_TEST_HISTORY_HOME", dir.path())
                .env("RARA_TEST_HISTORY_WRITER", worker.to_string())
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    for child in &mut children {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("history writer timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let loaded = store.load().unwrap();
    assert_eq!(loaded.entries.len(), MAX_HISTORY_ENTRIES);
    assert_eq!(loaded.skipped_lines, 0);
    for worker in 0..4 {
        for index in 0..20 {
            assert!(
                loaded
                    .entries
                    .iter()
                    .any(|entry| entry.text() == format!("worker {worker} prompt {index}"))
            );
        }
    }
}

#[test]
#[ignore = "isolated history writer, invoked by its parent test"]
fn process_writer() {
    let store = PromptHistoryStore::new(std::env::var_os("RARA_TEST_HISTORY_HOME").unwrap());
    let worker = std::env::var("RARA_TEST_HISTORY_WRITER").unwrap();
    for index in 0..20 {
        store
            .append(&entry(&format!("worker {worker} prompt {index}")))
            .unwrap();
    }
}
