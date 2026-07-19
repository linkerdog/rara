use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::Child;
use rara_tools::tool::Tool;
use serde_json::{Value, json};
use tempfile::tempdir;

use super::PtySessionStore;
use super::output::read_output_tail;
use super::store::{
    START_QUICK_COMPLETION_TIMEOUT as PTY_START_QUICK_COMPLETION_TIMEOUT, command_env_for_wrapped,
};
use super::types::{
    PtyKillTool, PtyListTool, PtyReadTool, PtySessionRecord, PtySessionStatus, PtyStartTool,
    PtyStatusTool, PtyStopTool, PtyWriteTool,
};
use crate::sandbox::{SandboxManager, WrappedCommand};

#[tokio::test]
async fn read_output_tail_returns_only_requested_suffix() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("pty.log");
    tokio::fs::write(&path, b"0123456789tail")
        .await
        .expect("write log");

    let output = read_output_tail(&path, 4).await.expect("tail");

    assert_eq!(output, "tail");
}

#[test]
fn pty_tool_schema_guides_interactive_command_discipline() {
    let temp = tempdir().expect("tempdir");
    let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));
    let start = PtyStartTool {
        sessions: sessions.clone(),
        sandbox: Arc::new(
            SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox"),
        ),
        base_env: Arc::new(HashMap::new()),
        sandbox_network_access: Arc::new(AtomicBool::new(false)),
    };
    let list = PtyListTool {
        sessions: sessions.clone(),
    };
    let status = PtyStatusTool {
        sessions: sessions.clone(),
    };
    let stop = PtyStopTool { sessions };

    let description = start.description();
    assert!(description.contains("interactive PTY session only"));
    assert!(description.contains("use bash instead"));
    assert!(description.contains("Prefer dedicated RARA tools"));
    assert!(description.contains("cwd field"));
    assert!(description.contains("sandboxing is platform-dependent"));
    assert!(description.contains("macOS seatbelt backend"));
    assert!(description.contains("allow_net as a network-access toggle"));
    assert!(description.contains("pty_status"));
    assert!(description.contains("pty_list"));
    assert!(description.contains("pty_stop"));

    let schema = start.input_schema().to_string();
    assert!(schema.contains("Use PTY only for interactive commands"));
    assert!(schema.contains("Do not prefix this command with cd"));
    assert!(schema.contains("use bash for ordinary non-interactive commands"));
    assert!(schema.contains("Use this instead of prefixing the command with cd"));
    assert!(list.description().contains("duplicate interactive work"));
    assert!(status.description().contains("pty_start"));
    assert!(stop.description().contains("session_id is omitted"));
}

#[test]
fn sandboxed_pty_env_falls_back_to_process_path_when_snapshot_path_is_missing() {
    let temp = tempdir().expect("tempdir");
    let wrapped = WrappedCommand {
        program: "bwrap".to_string(),
        args: vec!["--version".to_string()],
        cleanup_path: None,
        sandboxed: true,
        sandbox_backend: "linux-bubblewrap".to_string(),
        sandbox_home: Some(temp.path().join("home")),
        network_access: false,
    };
    let env_map = command_env_for_wrapped(
        &wrapped,
        &HashMap::from([("PATH".to_string(), String::new())]),
        &HashMap::new(),
    )
    .expect("pty env");

    assert!(
        env_map.get("PATH").is_some_and(|path| !path.is_empty()),
        "sandboxed PTY env must keep a usable PATH after env_clear"
    );
    assert_eq!(
        env_map
            .get("RARA_SANDBOX_NETWORK_DISABLED")
            .map(String::as_str),
        Some("1")
    );
}

#[tokio::test]
async fn pty_session_accepts_input_and_exposes_output() {
    let temp = tempdir().expect("tempdir");
    let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));
    let write = PtyWriteTool {
        sessions: sessions.clone(),
    };
    let read = PtyReadTool {
        sessions: sessions.clone(),
    };

    let command = "read line; printf \"got:%s\\n\" \"$line\"".to_string();
    let started = sessions
        .start(
            command.clone(),
            WrappedCommand {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), command],
                cleanup_path: None,
                sandboxed: false,
                sandbox_backend: "direct".to_string(),
                sandbox_home: None,
                network_access: true,
            },
            temp.path().display().to_string(),
            &HashMap::new(),
            HashMap::new(),
            24,
            120,
        )
        .expect("start pty")
        .into_json(12_000)
        .await
        .expect("pty json");
    let session_id = started
        .get("session_id")
        .and_then(Value::as_str)
        .expect("session id")
        .to_string();
    assert_eq!(
        started.get("network_access").and_then(Value::as_bool),
        Some(true)
    );

    write
        .call(json!({
            "session_id": session_id,
            "input": "hello from pty\n",
        }))
        .await
        .expect("write pty");

    let mut last = Value::Null;
    for _ in 0..50 {
        last = read
            .call(json!({ "session_id": session_id, "tail_bytes": 4096 }))
            .await
            .expect("read pty");
        if last
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("got:hello from pty")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let output = last
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        output.contains("got:hello from pty"),
        "last pty output did not contain expected marker: {last}"
    );
}

#[tokio::test]
async fn pty_start_waits_briefly_for_quick_command_output() {
    let temp = tempdir().expect("tempdir");
    let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));

    let command = "printf 'quick-done\\n'".to_string();
    let started = sessions
        .start(
            command.clone(),
            WrappedCommand {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), command],
                cleanup_path: None,
                sandboxed: false,
                sandbox_backend: "direct".to_string(),
                sandbox_home: None,
                network_access: true,
            },
            temp.path().display().to_string(),
            &HashMap::new(),
            HashMap::new(),
            24,
            120,
        )
        .expect("start pty");
    let inspected = sessions
        .wait_for_quick_completion(&started.id, PTY_START_QUICK_COMPLETION_TIMEOUT)
        .await
        .into_json(12_000)
        .await
        .expect("pty json");

    assert_eq!(
        inspected.get("status").and_then(Value::as_str),
        Some("completed")
    );
    assert!(
        inspected
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("quick-done"),
        "quick command output should be available in pty_start result: {inspected}"
    );
}

#[tokio::test]
async fn pty_start_keeps_long_running_session_running_after_brief_wait() {
    let temp = tempdir().expect("tempdir");
    let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));

    let command = "sleep 2".to_string();
    let started = sessions
        .start(
            command.clone(),
            WrappedCommand {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), command],
                cleanup_path: None,
                sandboxed: false,
                sandbox_backend: "direct".to_string(),
                sandbox_home: None,
                network_access: true,
            },
            temp.path().display().to_string(),
            &HashMap::new(),
            HashMap::new(),
            24,
            120,
        )
        .expect("start pty");
    let inspected = sessions
        .wait_for_quick_completion(&started.id, Duration::from_millis(100))
        .await;

    assert_eq!(inspected.status, PtySessionStatus::Running);
    sessions.kill(&started.id).expect("cleanup pty");
}

#[tokio::test]
async fn pty_kill_reports_killing_until_reader_observes_eof() {
    let temp = tempdir().expect("tempdir");
    let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));

    let command = "sleep 30".to_string();
    let started = sessions
        .start(
            command.clone(),
            direct_shell_wrapped(&command),
            temp.path().display().to_string(),
            &HashMap::new(),
            HashMap::new(),
            24,
            120,
        )
        .expect("start pty");

    let killing = sessions.kill(&started.id).expect("kill pty");

    assert_eq!(killing.status, PtySessionStatus::Killing);
    assert_eq!(
        wait_for_session_status(
            &sessions,
            &started.id,
            PtySessionStatus::Killed,
            Duration::from_secs(3)
        )
        .await,
        Some(PtySessionStatus::Killed)
    );
}

#[tokio::test]
async fn pty_kill_keeps_completed_session_completed() {
    let temp = tempdir().expect("tempdir");
    let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));

    let command = "printf done".to_string();
    let started = sessions
        .start(
            command.clone(),
            direct_shell_wrapped(&command),
            temp.path().display().to_string(),
            &HashMap::new(),
            HashMap::new(),
            24,
            120,
        )
        .expect("start pty");
    let completed = sessions
        .wait_for_quick_completion(&started.id, PTY_START_QUICK_COMPLETION_TIMEOUT)
        .await;

    assert_eq!(completed.status, PtySessionStatus::Completed);
    let stopped = sessions.kill(&started.id).expect("kill completed pty");
    assert_eq!(stopped.status, PtySessionStatus::Completed);
}

#[test]
fn pty_kill_error_restores_running_status() {
    let temp = tempdir().expect("tempdir");
    let sessions = PtySessionStore::new(temp.path().join("pty")).expect("pty store");
    let id = "pty-test".to_string();
    let status = Arc::new(Mutex::new(PtySessionStatus::Running));
    let record = PtySessionRecord {
        id: id.clone(),
        command: "failing".to_string(),
        output_path: temp.path().join("pty.log"),
        sandboxed: false,
        sandbox_backend: "direct".to_string(),
        network_access: true,
        writer: Arc::new(Mutex::new(Box::new(Vec::<u8>::new()))),
        child: Arc::new(Mutex::new(Box::new(FailingKillChild))),
        child_pid: None,
        status,
        last_read: Instant::now(),
    };
    sessions
        .sessions
        .lock()
        .expect("pty session store lock")
        .insert(id.clone(), record);

    let err = match sessions.kill(&id) {
        Ok(_) => panic!("kill should fail"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("permission denied"));
    assert_eq!(
        sessions.get(&id).map(|snapshot| snapshot.status),
        Some(PtySessionStatus::Running)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn pty_kill_terminates_background_children_in_process_group() {
    let temp = tempdir().expect("tempdir");
    let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));

    let marker = "__rara_bg_pid:";
    let command = format!("sleep 1000 & bg=$!; echo {marker}$bg; wait");
    let started = sessions
        .start(
            command.clone(),
            direct_shell_wrapped(&command),
            temp.path().display().to_string(),
            &HashMap::new(),
            HashMap::new(),
            24,
            120,
        )
        .expect("start pty");
    let bg_pid = wait_for_marker_pid(&started.output_path, marker, Duration::from_secs(3))
        .await
        .expect("background pid marker");
    assert!(
        process_is_active(bg_pid),
        "expected background child pid {bg_pid} to exist before kill"
    );

    let killing = sessions.kill(&started.id).expect("kill pty");

    assert_eq!(killing.status, PtySessionStatus::Killing);
    let inactive = wait_for_process_inactive(bg_pid, Duration::from_secs(3)).await;
    if !inactive {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(bg_pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    assert!(inactive, "background child pid {bg_pid} survived pty_kill");
    assert_eq!(
        wait_for_session_status(
            &sessions,
            &started.id,
            PtySessionStatus::Killed,
            Duration::from_secs(3)
        )
        .await,
        Some(PtySessionStatus::Killed)
    );
}

#[tokio::test]
async fn pty_sessions_can_be_listed_statused_and_stopped() {
    let temp = tempdir().expect("tempdir");
    let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));
    let list = PtyListTool {
        sessions: sessions.clone(),
    };
    let status = PtyStatusTool {
        sessions: sessions.clone(),
    };
    let stop = PtyStopTool {
        sessions: sessions.clone(),
    };

    let command = "sleep 30".to_string();
    let started = sessions
        .start(
            command.clone(),
            WrappedCommand {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), command],
                cleanup_path: None,
                sandboxed: false,
                sandbox_backend: "direct".to_string(),
                sandbox_home: None,
                network_access: true,
            },
            temp.path().display().to_string(),
            &HashMap::new(),
            HashMap::new(),
            24,
            120,
        )
        .expect("start pty")
        .into_json(12_000)
        .await
        .expect("pty json");
    let session_id = started
        .get("session_id")
        .and_then(Value::as_str)
        .expect("session id")
        .to_string();
    assert_eq!(
        started.get("network_access").and_then(Value::as_bool),
        Some(true)
    );

    let listed = list.call(json!({})).await.expect("list ptys");
    assert_eq!(
        listed
            .get("sessions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        listed
            .pointer("/sessions/0/network_access")
            .and_then(Value::as_bool),
        Some(true)
    );

    let inspected = status
        .call(json!({ "session_id": session_id }))
        .await
        .expect("pty status");
    assert_eq!(
        inspected.get("status").and_then(Value::as_str),
        Some("running")
    );
    assert_eq!(
        inspected.get("network_access").and_then(Value::as_bool),
        Some(true)
    );

    let stopped = stop.call(json!({})).await.expect("stop all ptys");
    assert_eq!(
        stopped.pointer("/stopped/0/status").and_then(Value::as_str),
        Some("killing")
    );
    assert_eq!(
        stopped
            .pointer("/stopped/0/network_access")
            .and_then(Value::as_bool),
        Some(true)
    );
}

fn direct_shell_wrapped(command: &str) -> WrappedCommand {
    WrappedCommand {
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), command.to_string()],
        cleanup_path: None,
        sandboxed: false,
        sandbox_backend: "direct".to_string(),
        sandbox_home: None,
        network_access: true,
    }
}

#[derive(Debug)]
struct FailingKillChild;

impl portable_pty::ChildKiller for FailingKillChild {
    fn kill(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        ))
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(FailingKillChild)
    }
}

impl Child for FailingKillChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        Ok(None)
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        Ok(portable_pty::ExitStatus::with_exit_code(1))
    }

    fn process_id(&self) -> Option<u32> {
        None
    }
}

async fn wait_for_session_status(
    sessions: &PtySessionStore,
    id: &str,
    expected: PtySessionStatus,
    timeout: Duration,
) -> Option<PtySessionStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        let status = sessions.get(id).map(|snapshot| snapshot.status);
        if status == Some(expected) || Instant::now() >= deadline {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(unix)]
async fn wait_for_marker_pid(path: &Path, marker: &str, timeout: Duration) -> Option<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        let output = read_output_tail(path, 4096).await.ok()?;
        if let Some(pid) = parse_marker_pid(&output, marker) {
            return Some(pid);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(unix)]
fn parse_marker_pid(output: &str, marker: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        let (_, tail) = line.split_once(marker)?;
        let pid = tail
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        pid.parse().ok()
    })
}

#[cfg(unix)]
fn process_is_active(pid: u32) -> bool {
    process_state(pid).is_some_and(|state| !state.starts_with('Z'))
}

#[cfg(unix)]
fn process_state(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!state.is_empty()).then_some(state)
}

#[cfg(unix)]
async fn wait_for_process_inactive(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !process_is_active(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
