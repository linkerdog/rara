use std::cell::Cell;
use std::path::PathBuf;

use tokio::task::JoinHandle;

use super::state::{Overlay, TuiApp};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub(crate) enum DiffNavigation {
    Rows(i32),
    Pages(i32),
    Start,
    End,
}

#[derive(Default)]
pub(crate) struct DiffView {
    pub text: String,
    pub offset: Cell<usize>,
    pub rows: Cell<usize>,
    pub height: Cell<usize>,
    pending: Option<JoinHandle<anyhow::Result<String>>>,
    session_id: String,
}

impl DiffView {
    pub(crate) fn navigate(&self, action: DiffNavigation) {
        let max = self.rows.get().saturating_sub(self.height.get());
        let current = self.offset.get().min(max);
        let offset = match action {
            DiffNavigation::Rows(delta) => current.saturating_add_signed(delta as isize),
            DiffNavigation::Pages(delta) => current.saturating_add_signed(
                (delta as isize).saturating_mul(self.height.get().max(1) as isize),
            ),
            DiffNavigation::Start => 0,
            DiffNavigation::End => max,
        };
        self.offset.set(offset.min(max));
    }

    pub(crate) fn close(&mut self) {
        if let Some(task) = self.pending.take() {
            task.abort();
        }
    }
}

impl Drop for DiffView {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) fn open(app: &mut TuiApp) {
    let cwd = PathBuf::from(&app.snapshot.cwd);
    app.diff_view = DiffView {
        text: "Collecting local changes...".into(),
        pending: Some(tokio::spawn(async move {
            super::runtime::review::capture_working_tree(&cwd).await
        })),
        session_id: app.snapshot.session_id.clone(),
        offset: Cell::new(0),
        rows: Cell::new(0),
        height: Cell::new(0),
    };
    app.open_overlay(Overlay::Diff);
}

pub(super) async fn poll(app: &mut TuiApp) -> bool {
    if !app
        .diff_view
        .pending
        .as_ref()
        .is_some_and(JoinHandle::is_finished)
    {
        return false;
    }
    let Some(task) = app.diff_view.pending.take() else {
        return false;
    };
    let result = task
        .await
        .map_err(anyhow::Error::from)
        .and_then(|result| result);
    if app.snapshot.session_id != app.diff_view.session_id {
        app.diff_view.text = "The thread changed. Run /diff again to refresh.".into();
    } else {
        app.diff_view.text = match result {
            Ok(text) => text,
            Err(error) => {
                log::warn!("Diff capture failed: {error:#}");
                format!("Could not collect changes: {error:#}")
            }
        };
    }
    true
}
