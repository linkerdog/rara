use std::io::{self, Read};

use super::*;

#[test]
fn live_recovery_retains_valid_records_around_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let entry = PersistedTurnEntry {
        role: "Agent".into(),
        message: "valid entry".into(),
    };
    append_rollout_fragment(dir.path(), "thread", &entry).unwrap();
    let path = dir.path().join("thread/live.jsonl");
    let valid = fs::read(&path).unwrap();
    let mut contents = valid.clone();
    contents.extend_from_slice(b"{secret-invalid}\n\xff\n \r\n");
    contents.extend_from_slice(&valid);
    contents.extend_from_slice(b"{truncated");
    fs::write(&path, &contents).unwrap();
    let loaded = load_live_entries_with_recovery(dir.path(), "thread");
    assert_eq!(loaded.entries.len(), 2);
    assert!(loaded.entries.iter().all(|e| e.message == entry.message));
    assert_eq!(loaded.skipped_lines, 3);
    assert!(loaded.read_error.is_none());
    assert_eq!(
        loaded.warning().unwrap(),
        "Live transcript recovery incomplete: 3 invalid record(s) skipped."
    );
    assert_eq!(fs::read(path).unwrap(), contents);
}

#[test]
fn missing_and_empty_live_logs_are_not_recovery_failures() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        load_live_entries_with_recovery(dir.path(), "thread")
            .warning()
            .is_none()
    );
    fs::create_dir_all(dir.path().join("thread")).unwrap();
    fs::write(dir.path().join("thread/live.jsonl"), b" \n\r\n").unwrap();
    let loaded = load_live_entries_with_recovery(dir.path(), "thread");
    assert!(loaded.entries.is_empty());
    assert!(loaded.warning().is_none());
}

#[test]
fn inaccessible_live_log_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("thread"), "not a directory").unwrap();
    let loaded = load_live_entries_with_recovery(dir.path(), "thread");
    assert!(loaded.entries.is_empty());
    assert!(loaded.read_error.is_some());
    assert!(
        loaded
            .warning()
            .unwrap()
            .contains("Could not open live transcript")
    );
}

struct FailingReader;

impl Read for FailingReader {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("injected read failure"))
    }
}

#[test]
fn read_failure_retains_the_prefix_and_stops_retrying() {
    let entry = PersistedTurnEntry {
        role: "Agent".into(),
        message: "prefix".into(),
    };
    let mut line = serde_json::to_vec(&entry).unwrap();
    line.push(b'\n');
    let reader = BufReader::new(io::Cursor::new(line).chain(FailingReader));
    let loaded = recover_live_entries(reader);
    assert_eq!(loaded.entries.len(), 1);
    assert_eq!(loaded.entries[0].message, "prefix");
    assert_eq!(
        loaded.read_error.as_deref(),
        Some("Could not read live transcript at line 2: injected read failure")
    );
}
