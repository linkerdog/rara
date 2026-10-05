use std::path::PathBuf;
use std::sync::Arc;

use super::document::save_recovery;
use crate::tui::composer_atoms::ComposerDraft;
use crate::tui::display_sanitize::sanitize_paste_text;
use crate::tui::state::{BottomPaneModel, TuiApp};

pub(in crate::tui) struct EditorRequest {
    pub seed: Arc<str>,
    pub cwd: PathBuf,
}

pub(in crate::tui) struct EditorDraft {
    original: ComposerDraft,
    seed: Arc<str>,
    session: String,
    cwd: String,
}

impl EditorDraft {
    pub fn capture(app: &mut TuiApp) -> (Self, EditorRequest) {
        app.flush_composer_paste();
        app.cancel_pending_history_navigation();
        let original = app.bottom_pane.saved_draft();
        let mut expanded = BottomPaneModel::new();
        expanded.restore_draft(original.clone());
        expanded.expand_owned_pastes();
        let seed = Arc::<str>::from(expanded.input);
        let request = EditorRequest {
            seed: seed.clone(),
            cwd: PathBuf::from(&app.snapshot.cwd),
        };
        (
            Self {
                original,
                seed,
                session: app.snapshot.session_id.clone(),
                cwd: app.snapshot.cwd.clone(),
            },
            request,
        )
    }

    pub async fn finish(self, app: &mut TuiApp, result: anyhow::Result<String>) {
        let edited = match result {
            Ok(text) => sanitize_paste_text(&text),
            Err(error) => {
                log::warn!("External editor failed: {error:#}");
                app.push_notice(format!("External editor: {error:#}"));
                return;
            }
        };
        if edited == self.seed.as_ref() {
            return;
        }
        if app.snapshot.session_id != self.session
            || app.snapshot.cwd != self.cwd
            || app.bottom_pane.saved_draft() != self.original
        {
            match save_recovery(edited).await {
                Ok(path) => app.push_notice(format!(
                    "Draft changed while editing; edited text saved to {}",
                    path.display()
                )),
                Err(error) => {
                    log::warn!("Could not preserve stale editor result: {error:#}");
                    app.push_notice(format!(
                        "Draft changed while editing; recovery failed: {error:#}"
                    ));
                }
            }
            return;
        }
        app.bottom_pane.clear_input();
        app.bottom_pane.edit_composer(0..0, &edited);
        if app.composer_input_is_active() {
            app.sync_command_palette_with_input();
        }
        app.reset_input_history_navigation();
        app.refresh_file_mentions();
    }
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
