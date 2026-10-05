use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ShellEnvironmentSnapshot {
    pub env: HashMap<String, String>,
    pub source: Option<String>,
}

const SHELL_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);
const PATH_START_MARKER: &str = "__RARA_PATH_START__";
const PATH_END_MARKER: &str = "__RARA_PATH_END__";

pub(crate) async fn capture_shell_environment_snapshot() -> ShellEnvironmentSnapshot {
    let Some(shell) = std::env::var_os("SHELL").and_then(|value| {
        let path = Path::new(&value);
        (path.is_absolute() && path.is_file()).then(|| path.to_path_buf())
    }) else {
        return process_environment_snapshot();
    };

    let shell_name = shell
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let script = if shell_name == "fish" {
        "echo __RARA_PATH_START__; string join : $PATH; echo __RARA_PATH_END__"
    } else {
        "printf '__RARA_PATH_START__%s__RARA_PATH_END__' \"$PATH\""
    };

    let mut command = Command::new(&shell);
    command
        .arg("-lc")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let Ok(Ok(output)) = timeout(SHELL_SNAPSHOT_TIMEOUT, command.output()).await else {
        log::warn!("Warning: failed to capture shell PATH snapshot; using process PATH");
        return process_environment_snapshot();
    };
    if !output.status.success() {
        log::warn!("Warning: shell PATH snapshot command failed; using process PATH");
        return process_environment_snapshot();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(path) = extract_marked_path(&stdout) else {
        log::warn!("Warning: shell PATH snapshot output was not marked; using process PATH");
        return process_environment_snapshot();
    };

    ShellEnvironmentSnapshot {
        env: HashMap::from([("PATH".to_string(), path)]),
        source: Some(shell.display().to_string()),
    }
}

fn extract_marked_path(output: &str) -> Option<String> {
    let start = output.find(PATH_START_MARKER)? + PATH_START_MARKER.len();
    let remainder = &output[start..];
    let end = remainder.find(PATH_END_MARKER)?;
    let path = remainder[..end].trim();
    (!path.is_empty()).then(|| path.to_string())
}

fn process_environment_snapshot() -> ShellEnvironmentSnapshot {
    process_environment_snapshot_from_path(std::env::var("PATH").ok())
}

fn process_environment_snapshot_from_path(path: Option<String>) -> ShellEnvironmentSnapshot {
    let env = path
        .map(|path| HashMap::from([("PATH".to_string(), path)]))
        .unwrap_or_default();
    ShellEnvironmentSnapshot { env, source: None }
}

#[cfg(test)]
mod tests {
    use super::{
        PATH_END_MARKER, PATH_START_MARKER, extract_marked_path,
        process_environment_snapshot_from_path,
    };

    #[test]
    fn extracts_marked_path_from_noisy_shell_output() {
        let output = format!(
            "welcome\n{PATH_START_MARKER}/snapshot/bin:/usr/bin{PATH_END_MARKER}\nnotice\n"
        );

        assert_eq!(
            extract_marked_path(&output).as_deref(),
            Some("/snapshot/bin:/usr/bin")
        );
    }

    #[test]
    fn rejects_unmarked_shell_output() {
        assert_eq!(extract_marked_path("/snapshot/bin:/usr/bin"), None);
    }

    #[test]
    fn process_snapshot_preserves_path_when_available() {
        let snapshot =
            process_environment_snapshot_from_path(Some("/snapshot/bin:/usr/bin".to_string()));

        assert_eq!(
            snapshot.env.get("PATH").map(String::as_str),
            Some("/snapshot/bin:/usr/bin")
        );
        assert_eq!(snapshot.source, None);
    }
}
