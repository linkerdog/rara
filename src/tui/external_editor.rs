mod command;
mod document;
mod draft;
mod handoff;
#[cfg(unix)]
mod signals;

pub(super) use command::EditorCommand;
pub(super) use document::PreparedEdit;
pub(super) use draft::{EditorDraft, EditorRequest};
pub(super) use handoff::edit_with_terminal;

#[cfg(all(test, unix))]
mod pty_tests;
