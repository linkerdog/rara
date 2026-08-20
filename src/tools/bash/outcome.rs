#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ProcessTermination {
    Exit {
        code: i32,
    },
    Signal {
        signal: i32,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<&'static str>,
    },
    Unknown,
}

impl ProcessTermination {
    pub(super) fn from_status(status: &ExitStatus) -> Self {
        if let Some(code) = status.code() {
            return Self::Exit { code };
        }

        #[cfg(unix)]
        if let Some(signal) = status.signal() {
            return Self::Signal {
                signal,
                name: signal_name(signal),
            };
        }

        Self::Unknown
    }

    pub(super) fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Exit { code } => Some(*code),
            Self::Signal { .. } | Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum SandboxFailure {
    PolicyDenied { backend: String },
    SandboxedProcessSignaled { backend: String },
}

pub(super) fn classify_sandbox_failure(
    termination: &ProcessTermination,
    sandboxed: bool,
    backend: &str,
    captured_output: &str,
) -> Option<SandboxFailure> {
    if !sandboxed || matches!(termination, ProcessTermination::Exit { code: 0 }) {
        return None;
    }
    if has_policy_denial_evidence(captured_output) {
        return Some(SandboxFailure::PolicyDenied {
            backend: backend.to_string(),
        });
    }
    if matches!(termination, ProcessTermination::Signal { .. }) {
        return Some(SandboxFailure::SandboxedProcessSignaled {
            backend: backend.to_string(),
        });
    }
    None
}

fn has_policy_denial_evidence(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    [
        "sandbox: violation",
        "operation not permitted",
        "permission denied",
        "read-only file system",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(unix)]
fn signal_name(signal: i32) -> Option<&'static str> {
    match signal {
        libc::SIGHUP => Some("SIGHUP"),
        libc::SIGINT => Some("SIGINT"),
        libc::SIGQUIT => Some("SIGQUIT"),
        libc::SIGILL => Some("SIGILL"),
        libc::SIGABRT => Some("SIGABRT"),
        libc::SIGFPE => Some("SIGFPE"),
        libc::SIGKILL => Some("SIGKILL"),
        libc::SIGSEGV => Some("SIGSEGV"),
        libc::SIGPIPE => Some("SIGPIPE"),
        libc::SIGALRM => Some("SIGALRM"),
        libc::SIGTERM => Some("SIGTERM"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn distinguishes_exit_codes_from_unix_signals() {
        let exited = ExitStatus::from_raw(7 << 8);
        let signaled = ExitStatus::from_raw(libc::SIGABRT);

        assert_eq!(
            ProcessTermination::from_status(&exited),
            ProcessTermination::Exit { code: 7 }
        );
        assert_eq!(
            ProcessTermination::from_status(&signaled),
            ProcessTermination::Signal {
                signal: libc::SIGABRT,
                name: Some("SIGABRT"),
            }
        );
    }

    #[test]
    fn policy_denial_requires_captured_evidence() {
        let termination = ProcessTermination::Exit { code: 1 };

        assert_eq!(
            classify_sandbox_failure(
                &termination,
                true,
                "macos-seatbelt",
                "sandbox-exec: operation not permitted",
            ),
            Some(SandboxFailure::PolicyDenied {
                backend: "macos-seatbelt".to_string(),
            })
        );
        assert_eq!(
            classify_sandbox_failure(&termination, true, "macos-seatbelt", "ordinary failure"),
            None
        );
    }

    #[test]
    fn signal_only_failure_does_not_claim_policy_denial() {
        let termination = ProcessTermination::Signal {
            signal: 6,
            name: Some("SIGABRT"),
        };

        assert_eq!(
            classify_sandbox_failure(&termination, true, "macos-seatbelt", ""),
            Some(SandboxFailure::SandboxedProcessSignaled {
                backend: "macos-seatbelt".to_string(),
            })
        );
    }
}
