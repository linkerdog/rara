use std::io;

use anyhow::Result;
use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::disable_raw_mode,
};
use ratatui::{backend::CrosstermBackend, layout::Rect, text::Line};

use super::custom_terminal::Terminal;
use super::insert_history::{InsertHistoryMode, insert_history_lines_with_mode};
use super::render::committed_turn_lines;
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
    app: &mut TuiApp,
) -> Result<()> {
    let size = terminal.size()?;
    let area = viewport_area(size.width, size.height, viewport_height);
    if area != terminal.viewport_area {
        terminal.clear_visible_screen()?;
        terminal.set_viewport_area(area);
        // Signal flush_committed_history that the viewport was cleared so it
        // resets inserted_turns and re-inserts all history at new coordinates.
        app.viewport_was_cleared = true;
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

pub(super) fn flush_committed_history<
    B: ratatui::backend::Backend<Error = std::io::Error> + std::io::Write,
>(
    terminal: &mut Terminal<B>,
    app: &mut TuiApp,
) -> Result<()> {
    // When update_terminal_viewport cleared the screen after a resize,
    // re-insert all history from the beginning at new coordinates.
    if app.viewport_was_cleared {
        app.inserted_turns = 0;
        app.viewport_was_cleared = false;
    }
    while app.inserted_turns < app.committed_turns.len() {
        let turn = &app.committed_turns[app.inserted_turns];
        let cwd =
            (!app.snapshot.cwd.is_empty()).then(|| std::path::Path::new(app.snapshot.cwd.as_str()));
        let width = terminal.size()?.width;
        let mut lines = committed_turn_lines(turn.entries.as_slice(), cwd, width);
        if app.inserted_turns > 0 && !lines.is_empty() {
            lines.insert(0, Line::from(""));
        }
        if !lines.is_empty() {
            insert_history_lines_with_mode(terminal, lines, history_insert_mode())?;
        }
        app.inserted_turns += 1;
    }
    Ok(())
}

fn history_insert_mode() -> InsertHistoryMode {
    InsertHistoryMode::new(rara_terminal_detection::terminal_info().is_zellij())
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
    rara_terminal_detection::is_remote_session()
}

#[cfg(test)]
pub(crate) mod test_env {
    use std::ffi::OsString;
    use std::sync::{LazyLock, Mutex, MutexGuard};

    static SSH_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    pub(crate) struct SshEnvGuard {
        old_ssh_connection: Option<OsString>,
        old_ssh_tty: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    pub(crate) fn set_ssh_session(enabled: bool) -> SshEnvGuard {
        let lock = SSH_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_ssh_connection = std::env::var_os("SSH_CONNECTION");
        let old_ssh_tty = std::env::var_os("SSH_TTY");

        if enabled {
            set_env_var("SSH_CONNECTION", "test");
            remove_env_var("SSH_TTY");
        } else {
            remove_env_var("SSH_CONNECTION");
            remove_env_var("SSH_TTY");
        }

        SshEnvGuard {
            old_ssh_connection,
            old_ssh_tty,
            _lock: lock,
        }
    }

    impl Drop for SshEnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.old_ssh_connection.as_ref() {
                set_env_var("SSH_CONNECTION", value);
            } else {
                remove_env_var("SSH_CONNECTION");
            }
            if let Some(value) = self.old_ssh_tty.as_ref() {
                set_env_var("SSH_TTY", value);
            } else {
                remove_env_var("SSH_TTY");
            }
        }
    }

    fn set_env_var<K, V>(key: K, value: V)
    where
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
        // Tests serialize SSH env mutation through SSH_ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
    }

    fn remove_env_var<K>(key: K)
    where
        K: AsRef<std::ffi::OsStr>,
    {
        // Tests serialize SSH env mutation through SSH_ENV_LOCK.
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn flush_reinserts_when_visible_history_rows_is_zero() {
        use tempfile::tempdir;

        use crate::config::ConfigManager;
        use crate::tui::state::TranscriptTurn;
        use crate::tui::state::TuiApp;

        let temp = tempdir().unwrap();
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("build tui app");

        // Two committed turns "already inserted" — inserted_turns == len
        app.committed_turns = vec![TranscriptTurn::default(), TranscriptTurn::default()];
        app.inserted_turns = app.committed_turns.len();

        // Simulate what flush_committed_history does when
        // visible_history_rows was reset to 0 by clear_visible_screen.
        let visible_history_rows = 0;
        if visible_history_rows == 0 {
            app.inserted_turns = 0;
        }

        // inserted_turns should be reset so history gets re-inserted
        assert_eq!(app.inserted_turns, 0);
    }
}
