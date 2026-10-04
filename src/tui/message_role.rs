use crate::tui::display_sanitize::sanitize_display_line;

/// Semantic roles for live transcript entries. Only storage restoration accepts labels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MessageRole {
    User,
    Agent,
    System,
    Runtime,
    Responding,
    Tool,
    ToolResult,
    ToolError,
    ToolProgress,
    Exploring,
    Planning,
    Running,
    Thinking,
    Todo,
    Download,
    TerminalEvent,
    Compaction,
    ShellApprovalCompleted,
    QuestionAnswered,
    PlanningQuestionAnswered,
    ExplorationQuestionAnswered,
    SubAgentQuestionAnswered,
    PlanDecision,
    /// Unrecognized historical labels remain display-only compatibility data.
    Legacy(String),
}

impl Default for MessageRole {
    fn default() -> Self {
        Self::Legacy(String::new())
    }
}

impl MessageRole {
    pub(crate) fn from_persisted(role: &str) -> Self {
        let role = sanitize_display_line(role);
        match role.as_str() {
            "You" => Self::User,
            "Agent" => Self::Agent,
            "System" => Self::System,
            "Runtime" => Self::Runtime,
            "Responding" => Self::Responding,
            "Tool" => Self::Tool,
            "Tool Result" => Self::ToolResult,
            "Tool Error" => Self::ToolError,
            "Tool Progress" => Self::ToolProgress,
            "Exploring" => Self::Exploring,
            "Planning" => Self::Planning,
            "Running" => Self::Running,
            "Thinking" => Self::Thinking,
            "Todo" => Self::Todo,
            "Download" => Self::Download,
            "Terminal Event" => Self::TerminalEvent,
            "Compaction" => Self::Compaction,
            "Shell Approval Completed" => Self::ShellApprovalCompleted,
            "Question Answered" => Self::QuestionAnswered,
            "Planning Question Answered" => Self::PlanningQuestionAnswered,
            "Exploration Question Answered" => Self::ExplorationQuestionAnswered,
            "Sub-agent Question Answered" => Self::SubAgentQuestionAnswered,
            "Plan Decision" => Self::PlanDecision,
            label => Self::Legacy(label.to_owned()),
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::User => "You",
            Self::Agent => "Agent",
            Self::System => "System",
            Self::Runtime => "Runtime",
            Self::Responding => "Responding",
            Self::Tool => "Tool",
            Self::ToolResult => "Tool Result",
            Self::ToolError => "Tool Error",
            Self::ToolProgress => "Tool Progress",
            Self::Exploring => "Exploring",
            Self::Planning => "Planning",
            Self::Running => "Running",
            Self::Thinking => "Thinking",
            Self::Todo => "Todo",
            Self::Download => "Download",
            Self::TerminalEvent => "Terminal Event",
            Self::Compaction => "Compaction",
            Self::ShellApprovalCompleted => "Shell Approval Completed",
            Self::QuestionAnswered => "Question Answered",
            Self::PlanningQuestionAnswered => "Planning Question Answered",
            Self::ExplorationQuestionAnswered => "Exploration Question Answered",
            Self::SubAgentQuestionAnswered => "Sub-agent Question Answered",
            Self::PlanDecision => "Plan Decision",
            Self::Legacy(label) => label,
        }
    }
}
