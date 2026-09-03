// Sub-agent display helpers — icons, labels, and role detection.
//
// Centralizes the distinction between spawn_agent, explore_agent,
// plan_agent, and team_create so tool labels show the right icon + color
// instead of generic "Tool Result" / "Delegate" text.
use ratatui::style::Color;

use crate::tools::agent::AgentActivitySnapshot;
use crate::tui::format::format_token_count;
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

/// Shared text and semantic style projection for one child-agent status row.
pub(crate) struct SubAgentActivityDisplay<'a> {
    activity: &'a AgentActivitySnapshot,
}

impl<'a> SubAgentActivityDisplay<'a> {
    pub(crate) fn new(activity: &'a AgentActivitySnapshot) -> Self {
        Self { activity }
    }

    pub(crate) fn marker_and_color(&self) -> (&'static str, Color) {
        match self.activity.status.as_str() {
            "running" => ("[>]", STATUS_WARNING),
            "failed" | "budget_limited" => ("[!]", STATUS_WARNING),
            "cancelled" | "stopped" => ("[-]", TEXT_MUTED),
            _ => ("[x]", STATUS_SUCCESS),
        }
    }

    pub(crate) fn sidebar_header(&self) -> String {
        let (marker, _) = self.marker_and_color();
        format!("  {marker} {} ({})", self.name(), self.activity.kind)
    }

    pub(crate) fn status_header(&self) -> String {
        let (marker, _) = self.marker_and_color();
        format!(
            "  {marker} {} ({}) · {}{}",
            self.name(),
            self.activity.kind,
            self.activity.status,
            self.route()
        )
    }

    pub(crate) fn progress_line(&self, indent: &str) -> String {
        let mut progress = format!(
            "{indent}{} tools · {} tokens",
            self.activity.tool_use_count,
            format_token_count(self.activity.total_tokens)
        );
        if let Some(activity) = self.latest_activity() {
            progress.push_str(" · ");
            progress.push_str(activity);
        }
        progress
    }

    fn name(&self) -> &str {
        self.activity
            .name
            .as_deref()
            .or_else(|| {
                self.activity
                    .path
                    .rsplit('/')
                    .next()
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or(&self.activity.agent_id)
    }

    fn latest_activity(&self) -> Option<&str> {
        self.activity
            .latest_activity
            .as_deref()
            .or(self.activity.error.as_deref())
            .or(self.activity.summary.as_deref())
    }

    fn route(&self) -> String {
        match (
            self.activity.provider.as_deref(),
            self.activity.model.as_deref(),
        ) {
            (Some(provider), Some(model)) => format!(" · {provider}/{model}"),
            (Some(provider), None) => format!(" · {provider}"),
            (None, Some(model)) => format!(" · {model}"),
            (None, None) => String::new(),
        }
    }
}
