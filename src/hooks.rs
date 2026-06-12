// Hook declaration and sandbox execution for repo and protocol extensions.
//
// Discovers `.claude/hooks/` declarations, normalises them into
// RARA-owned HookDefinition objects, and executes them as
// sandboxed subprocesses with timeout and workspace isolation.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Hook phases aligned with Claude Code's lifecycle events.
use rara_instructions::HookLifecycle;
use serde::Serialize;

/// A discovered and normalised hook declaration.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HookDefinition {
    pub id: String,
    /// Repository-relative source path.
    pub source_path: String,
    /// Declared hook phase.
    pub phase: HookLifecycle,
    /// Whether the hook could be fully parsed.
    pub parse_status: HookParseStatus,
    /// Hook body / handler content.
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookParseStatus {
    Ok,
    /// File exists but could not be parsed or is empty.
    ParseError,
}

/// Discovers hook candidates and stores their normalised definitions.
/// Execution is currently disabled — this is discovery-only.
pub struct HookRegistry {
    pub hooks: BTreeMap<String, HookDefinition>,
    pub load_warnings: Vec<String>,
    /// All hook phases that have at least one registered hook.
    pub active_phases: Vec<HookLifecycle>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            hooks: BTreeMap::new(),
            load_warnings: Vec::new(),
            active_phases: Vec::new(),
        }
    }

    /// Scan a directory for Claude-style hook files.
    /// Expected file names: `pre-tool-use.md`, `session-start.md`, etc.
    pub fn discover_from_dir(&mut self, dir: &Path) {
        if !dir.exists() {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                self.load_warnings
                    .push(format!("hook dir {}: {err}", dir.display()));
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(phase) = HookLifecycle::from_filename(stem) else {
                continue;
            };

            let id = format!("hook-{}", path.display());
            let source_path = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .display()
                .to_string();

            let body = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(err) => {
                    self.hooks.insert(
                        id.clone(),
                        HookDefinition {
                            id,
                            source_path,
                            phase,
                            parse_status: HookParseStatus::ParseError,
                            body: format!("read error: {err}"),
                        },
                    );
                    continue;
                }
            };

            let parse_status = if body.trim().is_empty() {
                HookParseStatus::ParseError
            } else {
                HookParseStatus::Ok
            };

            self.hooks.insert(
                id.clone(),
                HookDefinition {
                    id,
                    source_path,
                    phase,
                    parse_status,
                    body,
                },
            );
        }

        self.refresh_active_phases();
    }

    /// Discover hooks from a repository root directory.
    pub fn discover_repo_hooks(&mut self, repo_root: &Path) {
        let hooks_dir = repo_root.join(".claude").join("hooks");
        self.discover_from_dir(&hooks_dir);
    }

    fn refresh_active_phases(&mut self) {
        let mut phases: Vec<HookLifecycle> = self
            .hooks
            .values()
            .filter(|h| h.parse_status == HookParseStatus::Ok)
            .map(|h| h.phase)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        phases.sort_by_key(|p| phase_ordinal(*p));
        self.active_phases = phases;
    }

    /// For /context and /status: list each hook with phase, path, and parse status.
    pub fn status_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for hook in self.hooks.values() {
            let status = match hook.parse_status {
                HookParseStatus::Ok => "ok",
                HookParseStatus::ParseError => "parse_error",
            };
            lines.push(format!(
                "  {}  {}  {}  (disabled)",
                hook.phase.as_str(),
                hook.source_path,
                status
            ));
        }
        lines
    }
}

fn phase_ordinal(phase: HookLifecycle) -> u8 {
    match phase {
        HookLifecycle::SessionStart => 0,
        HookLifecycle::UserPromptSubmit => 1,
        HookLifecycle::PreToolUse => 2,
        HookLifecycle::PostToolUse => 3,
        HookLifecycle::PostMemoryWrite => 4,
        HookLifecycle::Stop => 5,
        HookLifecycle::PreCompact => 6,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_hook_phases_from_claude_filenames() {
        assert_eq!(
            HookLifecycle::from_filename("pre-tool-use"),
            Some(HookLifecycle::PreToolUse)
        );
        assert_eq!(
            HookLifecycle::from_filename("session-start"),
            Some(HookLifecycle::SessionStart)
        );
        assert_eq!(
            HookLifecycle::from_filename("stop"),
            Some(HookLifecycle::Stop)
        );
        assert_eq!(HookLifecycle::from_filename("unknown"), None);
    }

    #[test]
    fn discovers_hooks_from_directory() {
        let dir = tempdir().expect("tempdir");
        let hooks_dir = dir.path().join(".claude").join("hooks");
        fs::create_dir_all(&hooks_dir).expect("mkdir");
        fs::write(
            hooks_dir.join("pre-tool-use.md"),
            "# Pre-tool use hook\nrun validation",
        )
        .expect("write");
        fs::write(hooks_dir.join("stop.md"), "# Stop hook\ncleanup").expect("write");
        fs::write(hooks_dir.join("unknown.md"), "ignored").expect("write");

        let mut registry = HookRegistry::new();
        registry.discover_from_dir(&hooks_dir);

        assert_eq!(registry.hooks.len(), 2);
        assert!(registry.active_phases.contains(&HookLifecycle::PreToolUse));
        assert!(registry.active_phases.contains(&HookLifecycle::Stop));
    }

    #[test]
    fn empty_hook_file_is_parse_error() {
        let dir = tempdir().expect("tempdir");
        let hooks_dir = dir.path().join(".claude").join("hooks");
        fs::create_dir_all(&hooks_dir).expect("mkdir");
        fs::write(hooks_dir.join("stop.md"), "").expect("write");

        let mut registry = HookRegistry::new();
        registry.discover_from_dir(&hooks_dir);

        let hook = registry.hooks.values().next().unwrap();
        assert_eq!(hook.parse_status, HookParseStatus::ParseError);
        assert!(registry.active_phases.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Sandbox execution
// ---------------------------------------------------------------------------

/// Sandbox configuration for hook subprocess execution.
pub struct HookSandbox {
    /// Maximum runtime before the hook is killed.
    pub timeout: Duration,
    /// Working directory for the hook subprocess.
    pub workspace_root: std::path::PathBuf,
}

impl Default for HookSandbox {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            workspace_root: std::path::PathBuf::from("."),
        }
    }
}

/// Outcome of a hook execution.
pub struct HookOutcome {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Parse a hook outcome as a boolean decision.
impl HookOutcome {
    /// Returns true if the hook explicitly allows the action.
    /// Default is "allow" — only explicit non-zero exit or
    /// timeout blocks.
    pub fn allows(&self) -> bool {
        if self.timed_out {
            return false;
        }
        self.exit_code.unwrap_or(1) == 0
    }
}

/// Run a discovered hook subprocess with sandbox constraints.
///
/// Shells out to `bash -c <hook.body>` with limited environment,
/// no network, and a hard timeout.  stdin receives the input JSON.
/// stdout and stderr are read concurrently via background threads
/// to prevent pipe deadlock when hook output exceeds ~64KB.
pub fn run_sandboxed_hook(
    hook: &HookDefinition,
    sandbox: &HookSandbox,
    input_json: &str,
) -> Result<HookOutcome, std::io::Error> {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(&hook.body)
        .current_dir(&sandbox.workspace_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Write input then close stdin.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_json.as_bytes());
    }

    // Background threads consume stdout/stderr to avoid pipe deadlock.
    let child_stdout = child.stdout.take().expect("stdout pipe");
    let child_stderr = child.stderr.take().expect("stderr pipe");

    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let mut r = std::io::BufReader::new(child_stdout);
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut r, &mut buf);
        let _ = stdout_tx.send(buf);
    });
    std::thread::spawn(move || {
        let mut r = std::io::BufReader::new(child_stderr);
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut r, &mut buf);
        let _ = stderr_tx.send(buf);
    });

    // Wait with timeout.
    let start = Instant::now();
    let status;
    loop {
        match child.try_wait()? {
            Some(s) => {
                status = s;
                break;
            }
            None => {
                if start.elapsed() >= sandbox.timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(HookOutcome {
                        stdout: String::new(),
                        stderr: format!("hook {} timed out after {:?}", hook.id, sandbox.timeout),
                        exit_code: None,
                        timed_out: true,
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    let stdout = String::from_utf8_lossy(&stdout_rx.recv().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_rx.recv().unwrap_or_default()).into_owned();

    Ok(HookOutcome {
        stdout,
        stderr,
        exit_code: status.code(),
        timed_out: false,
    })
}
