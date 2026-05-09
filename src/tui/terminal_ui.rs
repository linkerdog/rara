use std::io;

use anyhow::Result;
use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::disable_raw_mode,
};
use ratatui::{backend::CrosstermBackend, layout::Rect};

use super::custom_terminal::Terminal;
use super::state::TuiApp;

pub(super) fn handle_paste(text: String, app: &mut TuiApp) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    app.insert_active_input_text(normalized.as_str());
}

pub(super) fn build_terminal(
    viewport_height: u16,
) -> Result<Terminal<CrosstermBackend<std::io::Stdout>>> {
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    execute!(terminal.backend_mut(), EnableBracketedPaste)?;

    let result = (|| -> Result<()> {
        let size = terminal.size()?;
        terminal.set_viewport_area(viewport_area(size.width, size.height, viewport_height));
        terminal.clear_visible_screen()?;
        Ok(())
    })();

    if let Err(err) = result {
        let _ = execute!(terminal.backend_mut(), DisableBracketedPaste);
        return Err(err);
    }

    Ok(terminal)
}

pub(super) fn update_terminal_viewport(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    viewport_height: u16,
    _app: &mut TuiApp,
) -> Result<()> {
    let size = terminal.size()?;
    let area = viewport_area(size.width, size.height, viewport_height);
    if area != terminal.viewport_area {
        terminal.clear_visible_screen()?;
        terminal.set_viewport_area(area);
    }
    Ok(())
}

pub(super) fn teardown_terminal(
    mut terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
) -> Result<()> {
    execute!(terminal.backend_mut(), DisableBracketedPaste)?;
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), Show)?;
    terminal.show_cursor()?;
    Ok(())
}

fn viewport_area(width: u16, height: u16, viewport_height: u16) -> Rect {
    let viewport_height = viewport_height.max(1).min(height.max(1));
    Rect::new(
        0,
        height.saturating_sub(viewport_height),
        width,
        viewport_height,
    )
}

pub(crate) fn is_ssh_session() -> bool {
    #[cfg(test)]
    if let Some(is_ssh) = test_env::ssh_session_override() {
        return is_ssh;
    }
    rara_terminal_detection::is_remote_session()
}

#[cfg(test)]
pub(crate) mod test_env {
    use std::ffi::OsString;
    use std::sync::{LazyLock, Mutex, MutexGuard};

    static SSH_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
    static SSH_SESSION_OVERRIDE: LazyLock<Mutex<Option<bool>>> = LazyLock::new(|| Mutex::new(None));

    pub(crate) struct SshEnvGuard {
        old_ssh_connection: Option<OsString>,
        old_ssh_tty: Option<OsString>,
        old_override: Option<bool>,
        _lock: MutexGuard<'static, ()>,
    }

    impl SshEnvGuard {
        pub(crate) fn set(is_ssh: bool) -> SshEnvGuard {
            let lock = SSH_ENV_LOCK
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let old_ssh_connection = std::env::var_os("SSH_CONNECTION");
            let old_ssh_tty = std::env::var_os("SSH_TTY");
            let old_override = {
                let mut override_guard = SSH_SESSION_OVERRIDE
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                override_guard.replace(is_ssh)
            };
            if is_ssh {
                unsafe {
                    std::env::set_var("SSH_CONNECTION", "1");
                }
            } else {
                unsafe {
                    std::env::remove_var("SSH_CONNECTION");
                }
            }
            unsafe {
                std::env::remove_var("SSH_TTY");
            }
            SshEnvGuard {
                old_ssh_connection,
                old_ssh_tty,
                old_override,
                _lock: lock,
            }
        }
    }

    impl Drop for SshEnvGuard {
        fn drop(&mut self) {
            {
                let mut override_guard = SSH_SESSION_OVERRIDE
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                *override_guard = self.old_override;
            }
            if let Some(ref val) = self.old_ssh_connection {
                unsafe {
                    std::env::set_var("SSH_CONNECTION", val);
                }
            } else {
                unsafe {
                    std::env::remove_var("SSH_CONNECTION");
                }
            }
            if let Some(ref val) = self.old_ssh_tty {
                unsafe {
                    std::env::set_var("SSH_TTY", val);
                }
            } else {
                unsafe {
                    std::env::remove_var("SSH_TTY");
                }
            }
        }
    }

    /// Convenience wrapper; returns a guard that auto-restores env on drop.
    pub(crate) fn set_ssh_session(is_ssh: bool) -> SshEnvGuard {
        SshEnvGuard::set(is_ssh)
    }

    pub(crate) fn ssh_session_override() -> Option<bool> {
        *SSH_SESSION_OVERRIDE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_ssh_guard_restoration() {
            let was_ssh = super::super::is_ssh_session();
            {
                let _guard = SshEnvGuard::set(true);
                assert!(super::super::is_ssh_session());
            }
            assert_eq!(super::super::is_ssh_session(), was_ssh);
        }
    }
}
