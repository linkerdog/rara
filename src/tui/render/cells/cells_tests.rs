#![allow(clippy::too_many_lines)]

use std::path::Path;

use insta::assert_snapshot;
use tempfile::tempdir;

use super::progress::ProgressRole;
pub(super) use super::progress::explicit_progress_entry_groups;
use super::{ActiveCell, ActiveTurnCell, CommittedTurnCell, HistoryCell};
use crate::config::ConfigManager;
use crate::tui::state::{RuntimePhase, RuntimeSnapshot, TranscriptEntry, TranscriptTurn, TuiApp};
use crate::tui::terminal_event::{TerminalCommandEvent, TerminalEvent, TerminalTarget};

#[path = "cells_tests/active_general.rs"]
mod active_general;
#[path = "cells_tests/active_plan.rs"]
mod active_plan;
#[path = "cells_tests/committed.rs"]
mod committed;
