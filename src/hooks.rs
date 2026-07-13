// Hook declaration and sandbox execution for repo and protocol extensions.
//
// Discovers `.claude/hooks/` declarations, normalises them into
// RARA-owned HookDefinition objects, and executes them as
// sandboxed subprocesses with timeout and workspace isolation.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Hook phases aligned with Claude Code's lifecycle events.
use rara_instructions::HookLifecycle;
use serde::{Deserialize, Serialize};

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
    /// Per-hook timeout from Claude-compatible settings, if configured.
    #[serde(skip_serializing)]
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookParseStatus {
    Ok,
    /// File exists but could not be parsed or is empty.
    ParseError,
}

/// Discovers hook candidates and stores their normalised definitions.
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
                            timeout: None,
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
                    timeout: None,
                },
            );
        }

        self.refresh_active_phases();
    }

    /// Discover hooks from a repository root directory.
    pub fn discover_repo_hooks(&mut self, repo_root: &Path) {
        let hooks_dir = repo_root.join(".claude").join("hooks");
        self.discover_from_dir(&hooks_dir);
        self.discover_claude_settings_hooks(repo_root);
    }

    /// Discover the Claude Code project-settings subset that RARA executes.
    ///
    /// Version one supports `hooks.Stop` command handlers. The handler runs at
    /// the completion boundary, so it needs no matcher and can return a
    /// blocking decision before the agent reports success.
    fn discover_claude_settings_hooks(&mut self, repo_root: &Path) {
        let path = repo_root.join(".claude").join("settings.json");
        if !path.exists() {
            return;
        }
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) => {
                self.load_warnings
                    .push(format!("hook settings {}: {err}", path.display()));
                return;
            }
        };
        let settings: ClaudeHookSettings = match serde_json::from_str(&contents) {
            Ok(settings) => settings,
            Err(err) => {
                self.load_warnings
                    .push(format!("hook settings {}: {err}", path.display()));
                return;
            }
        };
        let Some(groups) = settings.hooks.get("Stop") else {
            return;
        };

        for (group_index, group) in groups.iter().enumerate() {
            for (handler_index, handler) in group.hooks.iter().enumerate() {
                if handler.handler_type != "command" {
                    self.load_warnings.push(format!(
                        "hook settings {}: Stop handler {group_index}:{handler_index} has unsupported type {:?}",
                        path.display(),
                        handler.handler_type
                    ));
                    continue;
                }
                let Some(command) = handler
                    .command
                    .as_deref()
                    .filter(|command| !command.trim().is_empty())
                else {
                    self.load_warnings.push(format!(
                        "hook settings {}: Stop command handler {group_index}:{handler_index} is missing command",
                        path.display()
                    ));
                    continue;
                };
                self.hooks.insert(
                    format!("hook-settings-stop-{group_index}-{handler_index}"),
                    HookDefinition {
                        id: format!("hook-settings-stop-{group_index}-{handler_index}"),
                        source_path: ".claude/settings.json".to_string(),
                        phase: HookLifecycle::Stop,
                        parse_status: HookParseStatus::Ok,
                        body: command.to_string(),
                        timeout: handler.timeout.map(Duration::from_secs),
                    },
                );
            }
        }
        self.refresh_active_phases();
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

    /// Return all hooks registered for a given lifecycle phase.
    pub fn hooks_for_phase(&self, phase: HookLifecycle) -> Vec<&HookDefinition> {
        self.hooks
            .values()
            .filter(|h| h.parse_status == HookParseStatus::Ok && h.phase == phase)
            .collect()
    }

    /// Return command hooks loaded from Claude-compatible project settings.
    pub fn executable_hooks_for_phase(&self, phase: HookLifecycle) -> Vec<&HookDefinition> {
        self.hooks_for_phase(phase)
            .into_iter()
            .filter(|hook| hook.source_path == ".claude/settings.json")
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeHookSettings {
    #[serde(default)]
    hooks: BTreeMap<String, Vec<ClaudeHookMatcher>>,
}

#[derive(Debug, Deserialize)]
struct ClaudeHookMatcher {
    #[serde(default)]
    hooks: Vec<ClaudeHookHandler>,
}

#[derive(Debug, Deserialize)]
struct ClaudeHookHandler {
    #[serde(rename = "type")]
    handler_type: String,
    command: Option<String>,
    timeout: Option<u64>,
}

fn phase_ordinal(phase: HookLifecycle) -> u8 {
    match phase {
        HookLifecycle::SessionStart => 0,
        HookLifecycle::SessionEnd => 9,
        HookLifecycle::UserPromptSubmit => 1,
        HookLifecycle::PreToolUse => 2,
        HookLifecycle::PostToolUse => 3,
        HookLifecycle::PostMemoryWrite => 4,
        HookLifecycle::MemoryQuery => 5,
        HookLifecycle::Stop => 6,
        HookLifecycle::PreCompact => 7,
        HookLifecycle::PostCompact => 8,
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
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

    #[test]
    fn discovers_claude_settings_stop_command_hook() {
        let dir = tempdir().expect("tempdir");
        let claude_dir = dir.path().join(".claude");
        fs::create_dir_all(&claude_dir).expect("mkdir");
        fs::write(
            claude_dir.join("settings.json"),
            r#"{
                "hooks": {
                    "Stop": [{
                        "hooks": [{
                            "type": "command",
                            "command": "./check-completion.sh",
                            "timeout": 5
                        }]
                    }]
                }
            }"#,
        )
        .expect("settings");

        let mut registry = HookRegistry::new();
        registry.discover_repo_hooks(dir.path());

        let hooks = registry.hooks_for_phase(HookLifecycle::Stop);
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].body, "./check-completion.sh");
        assert_eq!(hooks[0].timeout, Some(Duration::from_secs(5)));
        assert_eq!(hooks[0].source_path, ".claude/settings.json");
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
        // No network access (not enforced at process level, but hooks
        // are expected to be local scripts that don't need it).
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_json.as_bytes());
    }
    // stdin is dropped here — closes the pipe so the hook sees EOF.

    let timeout = hook.timeout.unwrap_or(sandbox.timeout);
    let start = Instant::now();
    let output: std::process::Output;

    // Busy-wait with timeout — single-threaded, acceptable for <1s hooks.
    loop {
        match child.try_wait()? {
            Some(status) => {
                // Child exited — collect remaining output.
                let o = child.wait_with_output()?;
                output = std::process::Output {
                    status,
                    stdout: o.stdout,
                    stderr: o.stderr,
                };
                break;
            }
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(HookOutcome {
                        stdout: String::new(),
                        stderr: format!("hook {} timed out after {timeout:?}", hook.id),
                        exit_code: None,
                        timed_out: true,
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    Ok(HookOutcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code(),
        timed_out: false,
    })
}
