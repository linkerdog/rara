/// Message role enum replacing bare string comparisons like `role == "You"`.
/// Message roles used in transcript entries and render dispatch.
///
/// Centralizes the string-to-variant mapping to eliminate ~134 bare
/// `role == "You"` scattered across the TUI layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

impl MessageRole {
    pub(crate) fn try_from_str(role: &str) -> Option<Self> {
        match role {
            "You" => Some(Self::User),
            "Agent" => Some(Self::Agent),
            "System" => Some(Self::System),
            "Runtime" => Some(Self::Runtime),
            "Responding" => Some(Self::Responding),
            "Tool" => Some(Self::Tool),
            "Tool Result" => Some(Self::ToolResult),
            "Tool Error" => Some(Self::ToolError),
            "Tool Progress" => Some(Self::ToolProgress),
            "Exploring" => Some(Self::Exploring),
            "Planning" => Some(Self::Planning),
            "Running" => Some(Self::Running),
            "Thinking" => Some(Self::Thinking),
            "Todo" => Some(Self::Todo),
            _ => None,
        }
    }
}
