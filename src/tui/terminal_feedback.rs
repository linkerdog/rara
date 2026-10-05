use std::collections::HashSet;
use std::io::{self, Write};
use std::path::Path;

use super::display_sanitize::{bidi_annotation, sanitize_paste_text};
use super::state::{InteractionKind, TuiApp};
use super::terminal_control::TerminalTarget;
use crate::config::TerminalNotificationMethod;

mod title_stack;
pub(super) use title_stack::{TitleMode, restore_title, save_title};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TerminalNotification {
    Complete,
    Failed,
    InputRequired,
    ApprovalRequired,
}

impl TerminalNotification {
    fn message(self) -> &'static str {
        match self {
            Self::Complete => "RARA: turn complete",
            Self::Failed => "RARA: turn failed",
            Self::InputRequired => "RARA: input required",
            Self::ApprovalRequired => "RARA: approval required",
        }
    }
}

#[derive(Default)]
pub(crate) struct TerminalFeedbackState {
    title: Option<(String, String)>,
    pending: Option<(String, TerminalNotification)>,
    approval_session: Option<String>,
    seen_approvals: HashSet<String>,
    waiting_for_approval: bool,
}

impl TuiApp {
    pub(crate) fn set_terminal_thread_title(&mut self, title: Option<String>) {
        self.terminal_feedback.title = title.map(|title| (self.snapshot.session_id.clone(), title));
    }

    pub(crate) fn notify_terminal(&mut self, notification: TerminalNotification) {
        if self.terminal_focused
            || self.config.tui.terminal.notifications == TerminalNotificationMethod::Off
        {
            return;
        }
        let pending = &mut self.terminal_feedback.pending;
        if pending.as_ref().is_none_or(|(session, previous)| {
            session != &self.snapshot.session_id || notification > *previous
        }) {
            *pending = Some((self.snapshot.session_id.clone(), notification));
        }
    }

    pub(crate) fn notify_terminal_approval(&mut self, id: String) {
        if self.terminal_feedback.approval_session.as_ref() != Some(&self.snapshot.session_id) {
            self.terminal_feedback.approval_session = Some(self.snapshot.session_id.clone());
            self.terminal_feedback.seen_approvals.clear();
        }
        self.terminal_feedback.waiting_for_approval = true;
        if self.terminal_feedback.seen_approvals.insert(id) {
            // Remember focused events as well: blur must never revive them.
            self.notify_terminal(TerminalNotification::ApprovalRequired);
        }
    }

    pub(crate) fn clear_terminal_attention(&mut self) {
        self.terminal_feedback.waiting_for_approval = false;
        self.terminal_feedback.pending = None;
    }

    pub(crate) fn begin_terminal_query(&mut self) {
        self.clear_terminal_attention();
        self.terminal_feedback.seen_approvals.clear();
    }

    pub(crate) fn notify_terminal_query_complete(&mut self) {
        if self.is_busy() {
            return;
        }
        if let Some(pending) = self.active_pending_interaction() {
            let snapshot = pending._snapshot;
            match snapshot.kind {
                InteractionKind::Approval => {
                    if let Some(approval) = &snapshot.approval {
                        self.notify_terminal_approval(approval.tool_use_id.clone());
                    }
                }
                InteractionKind::PlanApproval => {
                    let fallback = format!("plan:{}", self.next_turn_ordinal);
                    let id = snapshot.source.as_deref().unwrap_or(&fallback);
                    self.notify_terminal_approval(
                        id.strip_prefix("exit_plan_mode:").unwrap_or(id).to_owned(),
                    );
                }
                InteractionKind::RequestInput => {
                    self.notify_terminal(TerminalNotification::InputRequired);
                }
            }
        } else {
            self.terminal_feedback.waiting_for_approval = false;
            self.notify_terminal(TerminalNotification::Complete);
        }
    }
}

pub(super) struct TerminalFeedback {
    title_mode: TitleMode,
    target: TerminalTarget,
    last_title: Option<String>,
}

impl TerminalFeedback {
    pub(super) fn new(title_mode: TitleMode, target: TerminalTarget) -> Self {
        Self {
            title_mode,
            target,
            last_title: None,
        }
    }

    pub(super) fn resumed(&mut self) {
        self.last_title = None;
    }

    pub(super) fn update(&mut self, app: &mut TuiApp, output: &mut dyn Write) -> io::Result<()> {
        let mut wrote = false;
        if self.title_mode == TitleMode::Enabled {
            let title = title_for(app);
            if self.last_title.as_ref() != Some(&title) {
                self.target.write(&format!("\x1b]2;{title}\x07"), output)?;
                self.last_title = Some(title);
                wrote = true;
            }
        }
        if let Some((session, notification)) = app.terminal_feedback.pending.take()
            && session == app.snapshot.session_id
            && !app.terminal_focused
        {
            match app.config.tui.terminal.notifications {
                TerminalNotificationMethod::Off => {}
                TerminalNotificationMethod::Bell => {
                    output.write_all(b"\x07")?;
                    wrote = true;
                }
                TerminalNotificationMethod::Osc9 => {
                    self.target
                        .write(&format!("\x1b]9;{}\x07", notification.message()), output)?;
                    wrote = true;
                }
            }
        }
        if wrote {
            output.flush()?;
        }
        Ok(())
    }
}

fn title_for(app: &TuiApp) -> String {
    let state = if app.has_pending_approval()
        || app.has_pending_plan_approval()
        || (app.terminal_feedback.waiting_for_approval
            && app.terminal_feedback.approval_session.as_ref() == Some(&app.snapshot.session_id))
    {
        "needs approval"
    } else if app.active_pending_interaction().is_some() {
        "needs input"
    } else if app.is_busy() {
        "running"
    } else {
        "idle"
    };
    let workspace = Path::new(&app.snapshot.cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&app.snapshot.cwd);
    let unnamed: String = app.snapshot.session_id.chars().take(8).collect();
    let thread = app
        .terminal_feedback
        .title
        .as_ref()
        .filter(|(session, _)| session == &app.snapshot.session_id)
        .map(|(_, title)| title.as_str())
        .unwrap_or(&unnamed);
    // Sanitize components independently so an unterminated escape in a path
    // cannot swallow the thread identity. Reserve space for both labels.
    let workspace: String = sanitize_title(workspace).chars().take(60).collect();
    let thread: String = sanitize_title(thread).chars().take(120).collect();
    format!("[{state}] {workspace} / {thread} - RARA")
}

fn sanitize_title(input: &str) -> String {
    sanitize_paste_text(input)
        .chars()
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .filter(|ch| !ch.is_control() && bidi_annotation(*ch).is_none())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}
