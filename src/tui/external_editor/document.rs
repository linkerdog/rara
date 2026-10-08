use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result};
use tempfile::TempDir;
use tokio::process::Command;

use super::command::EditorCommand;
use super::draft::EditorRequest;

pub(in crate::tui) struct PreparedEdit {
    directory: TempDir,
    path: PathBuf,
    command: EditorCommand,
    cwd: PathBuf,
}

impl PreparedEdit {
    pub async fn prepare(request: EditorRequest, command: EditorCommand) -> Result<Self> {
        tokio::task::spawn_blocking(move || {
            let directory = tempfile::Builder::new()
                .prefix("rara-editor-")
                .tempdir()
                .context("Create private editor directory")?;
            let path = directory.path().join("prompt.md");
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options
                .open(&path)?
                .write_all(request.seed.as_bytes())
                .context("Write editor draft")?;
            Ok(Self {
                directory,
                path,
                command,
                cwd: request.cwd,
            })
        })
        .await
        .context("Editor preparation worker failed")?
    }

    pub async fn run(self) -> Result<String> {
        let mut command = Command::new(&self.command.program);
        command
            .args(&self.command.arguments)
            .arg(&self.path)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if !self.cwd.as_os_str().is_empty() {
            command.current_dir(&self.cwd);
        }
        #[cfg(unix)]
        super::signals::configure_child(&mut command);
        let result = async {
            let status = command
                .status()
                .await
                .context("Launch or wait for editor")?;
            anyhow::ensure!(
                status.success(),
                "Editor exited with {status}; draft preserved"
            );
            let path = self.path.clone();
            tokio::task::spawn_blocking(move || std::fs::read_to_string(path))
                .await
                .context("Editor readback worker failed")?
                .context("Read edited UTF-8 draft")
        }
        .await;
        let path = self.directory.path().to_path_buf();
        let cleanup = tokio::task::spawn_blocking(move || self.directory.close()).await;
        match cleanup {
            Ok(Ok(())) => result,
            Ok(Err(error)) => {
                if let Err(primary) = &result {
                    log::warn!("Editor operation also failed before cleanup: {primary:#}");
                }
                log::warn!(
                    "Could not clean editor directory {}: {error}",
                    path.display()
                );
                Err(error).with_context(|| {
                    format!(
                        "Could not clean editor directory {}; draft preserved",
                        path.display()
                    )
                })
            }
            Err(error) => Err(error).context("Editor cleanup worker failed; draft preserved"),
        }
    }
}

pub(super) async fn save_recovery(text: String) -> Result<PathBuf> {
    tokio::task::spawn_blocking(move || {
        let mut file = tempfile::Builder::new()
            .prefix("rara-editor-recovery-")
            .suffix(".md")
            .tempfile()?;
        file.write_all(text.as_bytes())?;
        let (_file, path) = file.keep()?;
        anyhow::Ok(path)
    })
    .await
    .context("Editor recovery worker failed")?
}

#[cfg(all(test, unix))]
#[path = "document_tests.rs"]
mod tests;
