// Sub-agent display helpers — icons, labels, and role detection.
//
// Centralizes the distinction between spawn_agent, explore_agent,
// plan_agent, and team_create so tool labels show the right icon + color
// instead of generic "Tool Result" / "Delegate" text.
use ratatui::style::Color;

use crate::tui::theme::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubAgentKind {
    General, // spawn_agent
    Explore,
    Plan,
    Team,
}

impl SubAgentKind {
    pub(crate) fn from_tool_name(name: &str) -> Option<Self> {
        match name {
            "spawn_agent" => Some(Self::General),
            "explore_agent" => Some(Self::Explore),
            "plan_agent" => Some(Self::Plan),
            "team_create" => Some(Self::Team),
            _ => None,
        }
    }

    /// Icon + color for the tool action label shown when the tool starts.
    pub(crate) fn action_icon(self) -> (&'static str, Color) {
        match self {
            Self::General => ("🤖 ", ROLE_PREFIX),
            Self::Explore => ("🔍 ", PHASE_EXPLORING),
            Self::Plan => ("📋 ", PHASE_PLANNING),
            Self::Team => ("👥 ", STATUS_INFO),
        }
    }

    /// Action label text for the compact transcript summary.
    pub(crate) fn action_label(self) -> &'static str {
        match self {
            Self::General => "Delegate",
            Self::Explore => "Explore",
            Self::Plan => "Plan",
            Self::Team => "Team",
        }
    }
}

/// Color for the "Sub-agent Question" interaction card.
/// Distinct from the generic green RequestInput.
pub(crate) const SUB_AGENT_QUESTION_COLOR: Color = INTERACTION_SUB_AGENT;
