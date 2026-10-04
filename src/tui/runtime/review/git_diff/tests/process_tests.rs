use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

use super::*;

fn fake_git(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("git-fixture");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[tokio::test]
async fn failed_unstaged_capture_does_not_return_the_successful_staged_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let git = fake_git(
        dir.path(),
        r#"
for arg in "$@"; do
    if [ "$arg" = --staged ]; then printf 'staged change\n'; exit 0; fi
done
printf 'partial output that must not be reviewed\n'
printf 'fatal: scripted invalid index\n' >&2
exit 128
"#,
    );
    let error = capture_git_diff(dir.path(), git.as_os_str(), CaptureLimits::default())
        .await
        .err()
        .unwrap();
    let message = format!("{error:#}");
    assert!(message.contains("git diff failed"));
    assert!(message.contains("fatal: scripted invalid index"));
    assert!(!message.contains("staged change"));
}

#[tokio::test]
async fn large_stdout_and_stderr_are_drained_without_hiding_the_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let git = fake_git(
        dir.path(),
        r#"
head -c 1048576 /dev/zero
printf 'fatal: too much detail ' >&2
head -c 1048576 /dev/zero >&2
exit 7
"#,
    );
    let error = capture_git_diff(
        dir.path(),
        git.as_os_str(),
        CaptureLimits {
            diff_bytes: 64,
            stderr_bytes: 32,
            ..CaptureLimits::default()
        },
    )
    .await
    .err()
    .unwrap();
    let message = format!("{error:#}");
    assert!(message.contains("fatal: too much detail"));
    assert!(message.contains("stderr truncated"));
    assert!(message.len() < 256, "{message}");
}

#[tokio::test]
async fn combined_diff_bound_and_utf8_boundaries_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let git = fake_git(dir.path(), "printf 'abcdefgλrest'");
    let diff = capture_git_diff(
        dir.path(),
        git.as_os_str(),
        CaptureLimits {
            diff_bytes: 8,
            ..CaptureLimits::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(diff.text, "abcdefg");
    assert!(diff.truncated);
}

struct Cleanup(Option<Pid>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let Some(pid) = self.0.take() else {
            return;
        };
        match kill(pid, Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => {}
            Err(error) => log::warn!("Failed to clean up Git fixture: {error}"),
        }
    }
}

#[tokio::test]
async fn deadline_and_abort_terminate_and_reap_the_owned_git_process() {
    for abort in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let git = fake_git(dir.path(), "printf '%s' \"$$\" > git.pid\nexec sleep 60");
        let cwd = dir.path().to_path_buf();
        let task = tokio::spawn(async move {
            capture_git_diff(
                &cwd,
                git.as_os_str(),
                CaptureLimits {
                    deadline: Duration::from_secs(60),
                    ..CaptureLimits::default()
                },
            )
            .await
        });
        let pid = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match std::fs::read_to_string(dir.path().join("git.pid")) {
                    Ok(text) if !text.is_empty() => break Pid::from_raw(text.parse().unwrap()),
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => panic!("fixture readiness: {error}"),
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let mut cleanup = Cleanup(Some(pid));
        if abort {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(60)).await;
            let error = task.await.unwrap().err().unwrap();
            assert!(format!("{error:#}").contains("timed out"));
            tokio::time::resume();
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match kill(pid, None) {
                    Err(Errno::ESRCH) => break,
                    Ok(()) => tokio::time::sleep(Duration::from_millis(10)).await,
                    Err(error) => panic!("fixture liveness: {error}"),
                }
            }
        })
        .await
        .expect("Git must be terminated and reaped");
        cleanup.0 = None;
    }
}
