// Bottom pane view — pre-computed structured data consumed by renderers.
//
// Built once per frame from TuiApp by compute_bottom_pane_view(),
// so individual render modules (activity, composer, footer) never
// read TuiApp state directly.

use ratatui::{style::Color, text::Line};

use super::super::super::state::Overlay;
use super::super::super::state::RalphGoal;

/// Pre-computed data for one bottom-pane render frame.
pub(crate) struct BottomPaneView {
    pub(crate) activity: ActivityView,
    pub(crate) composer: ComposerInputView,
    pub(crate) footer: FooterView,
}

pub(crate) struct ActivityView {
    pub(crate) label: String,
    pub(crate) label_color: Color,
    pub(crate) detail: String,
    pub(crate) plan_badge: bool,
    pub(crate) perm_badge: bool,
    pub(crate) perm_label: &'static str,
    pub(crate) goal: Option<RalphGoal>,
}

#[allow(dead_code)]
pub(crate) struct ComposerInputView {
    pub(crate) input: String,
    pub(crate) cursor_offset: usize,
    pub(crate) scroll: usize,
    pub(crate) hint: Line<'static>,
    pub(crate) overlay: Option<Overlay>,
}

pub(crate) struct FooterView {
    pub(crate) parts: Vec<String>,
    pub(crate) hide: bool,
}
