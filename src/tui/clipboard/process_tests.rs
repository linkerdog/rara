use std::path::PathBuf;

use async_trait::async_trait;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

use super::*;

struct StalledHelper {
    scenario: &'static str,
    pid_file: PathBuf,
}

#[async_trait]
impl NativeClipboard for StalledHelper {
    async fn copy(&self, text: &str) -> io::Result<()> {
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command.args([
            "--exact",
            "tui::clipboard::process_tests::stalled_helper_child",
            "--ignored",
            "--nocapture",
        ]);
        command.env("RARA_CLIPBOARD_SCENARIO", self.scenario);
        command.env("RARA_CLIPBOARD_PID_FILE", &self.pid_file);
        native::pipe_to_command(&mut command, text).await
    }
}

struct HelperCleanup(Option<Pid>);

impl Drop for HelperCleanup {
    fn drop(&mut self) {
        let Some(pid) = self.0 else {
            return;
        };
        match kill(pid, Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => eprintln!("Failed to clean up clipboard fixture: {error}"),
        }
    }
}

#[tokio::test]
async fn timeout_terminates_and_reaps_helpers_stalled_on_stdin_or_exit() {
    for scenario in ["stdin", "exit"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("helper.pid");
        let mut clipboard = Clipboard::new(ClipboardOptions {
            target: TerminalTarget::Direct,
            writer: Box::new(Vec::new()),
            native: Some(Arc::new(StalledHelper {
                scenario,
                pid_file: pid_file.clone(),
            })),
            timeout: NATIVE_COPY_TIMEOUT,
        });
        clipboard.request("x".repeat(2 * 1024 * 1024));
        let pid = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match std::fs::read_to_string(&pid_file) {
                    Ok(text) => {
                        if let Ok(pid) = text.parse::<i32>() {
                            break Pid::from_raw(pid);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => panic!("helper identity: {error}"),
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("helper startup");
        let mut cleanup = HelperCleanup(Some(pid));
        let notice = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(notice) = clipboard.poll().await {
                    break notice;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("bounded copy");
        assert!(
            notice.contains("native clipboard copy timed out"),
            "{scenario}: {notice}"
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match kill(pid, None) {
                    Err(nix::errno::Errno::ESRCH) => break,
                    Ok(()) => tokio::time::sleep(Duration::from_millis(10)).await,
                    Err(error) => panic!("helper liveness: {error}"),
                }
            }
        })
        .await
        .expect("timed-out helper must be terminated and reaped");
        cleanup.0 = None;
    }
}

#[test]
#[ignore = "clipboard helper subprocess fixture"]
fn stalled_helper_child() {
    use std::io::Read;

    let scenario = std::env::var("RARA_CLIPBOARD_SCENARIO").expect("scenario");
    let pid_file = std::env::var_os("RARA_CLIPBOARD_PID_FILE").expect("pid file");
    std::fs::write(pid_file, std::process::id().to_string()).expect("record helper identity");
    match scenario.as_str() {
        "stdin" => {}
        "exit" => {
            io::stdin()
                .read_to_end(&mut Vec::new())
                .expect("consume selection");
        }
        other => panic!("unknown scenario: {other}"),
    }
    loop {
        std::thread::park();
    }
}
