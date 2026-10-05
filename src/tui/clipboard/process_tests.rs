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
    #[expect(
        clippy::print_stderr,
        reason = "Isolated subprocess cleanup reports failures to the test runner."
    )]
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
async fn timeout_and_session_drop_terminate_and_reap_stalled_helpers() {
    #[derive(Debug)]
    enum EndCopy {
        Timeout,
        SessionDrop,
    }
    for (scenario, end) in [
        ("stdin", EndCopy::Timeout),
        ("exit", EndCopy::Timeout),
        ("stdin", EndCopy::SessionDrop),
        ("exit", EndCopy::SessionDrop),
    ] {
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
        match end {
            EndCopy::Timeout => {
                let notice = tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        if let Some(notice) = clipboard.poll().await {
                            break notice.message;
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
            }
            EndCopy::SessionDrop => drop(clipboard),
        }
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
        .unwrap_or_else(|_| panic!("{end:?} ({scenario}) must terminate and reap the helper"));
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
