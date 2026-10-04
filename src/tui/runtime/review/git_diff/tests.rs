use super::*;

#[tokio::test]
async fn missing_git_is_an_error_and_not_a_clean_diff() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-git");
    let error = capture_git_diff(dir.path(), missing.as_os_str(), CaptureLimits::default())
        .await
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("could not start Git"));
}

#[tokio::test]
async fn a_non_repository_preserves_git_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let error = GitDiffCapture.capture(dir.path()).await.err().unwrap();
    let message = format!("{error:#}");
    assert!(message.contains("git diff --staged failed"), "{message}");
    assert!(message.contains("Git exited"), "{message}");
    assert!(!message.ends_with(": "), "stderr must not be discarded");
}

#[tokio::test]
async fn a_clean_repository_is_distinct_from_failed_capture() {
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .arg(dir.path())
        .status()
        .await
        .unwrap();
    assert!(status.success());
    let diff = GitDiffCapture.capture(dir.path()).await.unwrap();
    assert!(diff.text.is_empty());
    assert!(!diff.truncated);
}

#[tokio::test]
async fn bounded_reads_drain_beyond_the_retained_prefix() {
    let (mut writer, reader) = tokio::io::duplex(32);
    let producer = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        for _ in 0..128 {
            writer.write_all(b"abcdefgh").await.unwrap();
        }
    });
    let output = read_capped(reader, 7).await.unwrap();
    producer.await.unwrap();
    assert_eq!(output.bytes, b"abcdefg");
    assert!(output.truncated);
}

#[cfg(unix)]
mod process_tests;
