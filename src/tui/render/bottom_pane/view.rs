// Bottom pane view — pre-computed structured data consumed by renderers.
//
// Built once per frame from TuiApp by build_bottom_pane_view(),
// so individual render modules (activity, footer) never
// read TuiApp state directly.

use std::time::Duration;

use ratatui::style::Color;

/// Pre-computed data for one bottom-pane render frame.
pub(crate) struct BottomPaneView {
    pub(crate) activity: ActivityView,
    /// Approval / question panel rendered above the composer.
    pub(crate) interaction_panel: Option<InteractionPanelView>,
    pub(crate) footer: FooterView,
}

pub(crate) struct InteractionPanelView {
    pub(crate) title: &'static str,
    pub(crate) detail: String,
    pub(crate) actions: Vec<InteractionAction>,
    pub(crate) selected: usize,
}

pub(crate) struct InteractionAction {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
}

pub(crate) struct ActivityView {
    pub(crate) label: &'static str,
    pub(crate) label_color: Color,
    pub(crate) spinner: bool,
    pub(crate) spinner_elapsed: Duration,
    pub(crate) detail: String,
    pub(crate) plan_badge: bool,
    pub(crate) perm_badge: bool,
    pub(crate) perm_label: &'static str,
    pub(crate) goal_label: Option<(&'static str, Color)>,
    pub(crate) goal_detail: Option<String>,
}

pub(crate) struct FooterView {
    pub(crate) text: String,
    pub(crate) hide: bool,
}
