// Semantic color tokens for the TUI.
//
// Render-layer code should prefer ThemeToken lookups over raw
// ratatui::style::Color values so the user can override the visible palette
// without changing renderer internals. The legacy constants below remain as
// the default Nord-compatible palette and as fallback values for renderers that
// have not been migrated yet.
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use ratatui::style::Color;

use crate::config::TuiThemeConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum ThemeToken {
    UiElementBg,
    PopupBg,
    PopupDimmerBg,
    TextPrimary,
    TextSecondary,
    TextAccent,
    TextMuted,
    PickerItemFg,
    PickerItemMutedFg,
    PickerHighlightFg,
    PickerHighlightBg,
    RoleUser,
    RoleAgent,
    RoleSystem,
    RolePrefix,
    PhaseExploring,
    PhaseExplored,
    PhasePlanning,
    PhaseRunning,
    PhaseRan,
    StatusSuccess,
    StatusWarning,
    StatusError,
    StatusReady,
    StatusInfo,
    SurfaceBottomPaneBg,
    BadgeFgDark,
    InteractionSubAgent,
    PendingCardFg,
    ToolStderrFg,
    DiffAddBg,
    DiffAddFg,
    DiffDelBg,
    DiffDelFg,
    DiffHunkBg,
    DiffHunkFg,
    DiffContextFg,
    MarkdownHeading,
    MarkdownLink,
    MarkdownCode,
    MarkdownBlockQuote,
    MarkdownListBullet,
    MarkdownBold,
    MarkdownItalic,
    BudgetSystem,
    BudgetWorkspace,
    BudgetActive,
    BudgetHistory,
    BudgetMemory,
    BudgetOutput,
    BudgetFree,
}

const THEME_TOKENS: &[ThemeToken] = &[
    ThemeToken::UiElementBg,
    ThemeToken::PopupBg,
    ThemeToken::PopupDimmerBg,
    ThemeToken::TextPrimary,
    ThemeToken::TextSecondary,
    ThemeToken::TextAccent,
    ThemeToken::TextMuted,
    ThemeToken::PickerItemFg,
    ThemeToken::PickerItemMutedFg,
    ThemeToken::PickerHighlightFg,
    ThemeToken::PickerHighlightBg,
    ThemeToken::RoleUser,
    ThemeToken::RoleAgent,
    ThemeToken::RoleSystem,
    ThemeToken::RolePrefix,
    ThemeToken::PhaseExploring,
    ThemeToken::PhaseExplored,
    ThemeToken::PhasePlanning,
    ThemeToken::PhaseRunning,
    ThemeToken::PhaseRan,
    ThemeToken::StatusSuccess,
    ThemeToken::StatusWarning,
    ThemeToken::StatusError,
    ThemeToken::StatusReady,
    ThemeToken::StatusInfo,
    ThemeToken::SurfaceBottomPaneBg,
    ThemeToken::BadgeFgDark,
    ThemeToken::InteractionSubAgent,
    ThemeToken::PendingCardFg,
    ThemeToken::ToolStderrFg,
    ThemeToken::DiffAddBg,
    ThemeToken::DiffAddFg,
    ThemeToken::DiffDelBg,
    ThemeToken::DiffDelFg,
    ThemeToken::DiffHunkBg,
    ThemeToken::DiffHunkFg,
    ThemeToken::DiffContextFg,
    ThemeToken::MarkdownHeading,
    ThemeToken::MarkdownLink,
    ThemeToken::MarkdownCode,
    ThemeToken::MarkdownBlockQuote,
    ThemeToken::MarkdownListBullet,
    ThemeToken::MarkdownBold,
    ThemeToken::MarkdownItalic,
    ThemeToken::BudgetSystem,
    ThemeToken::BudgetWorkspace,
    ThemeToken::BudgetActive,
    ThemeToken::BudgetHistory,
    ThemeToken::BudgetMemory,
    ThemeToken::BudgetOutput,
    ThemeToken::BudgetFree,
];

impl ThemeToken {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::UiElementBg => "ui.element.bg",
            Self::PopupBg => "popup.bg",
            Self::PopupDimmerBg => "popup.dimmer.bg",
            Self::TextPrimary => "text.primary",
            Self::TextSecondary => "text.secondary",
            Self::TextAccent => "text.accent",
            Self::TextMuted => "text.muted",
            Self::PickerItemFg => "picker.item.fg",
            Self::PickerItemMutedFg => "picker.item.muted.fg",
            Self::PickerHighlightFg => "picker.highlight.fg",
            Self::PickerHighlightBg => "picker.highlight.bg",
            Self::RoleUser => "role.user",
            Self::RoleAgent => "role.agent",
            Self::RoleSystem => "role.system",
            Self::RolePrefix => "role.prefix",
            Self::PhaseExploring => "phase.exploring",
            Self::PhaseExplored => "phase.explored",
            Self::PhasePlanning => "phase.planning",
            Self::PhaseRunning => "phase.running",
            Self::PhaseRan => "phase.ran",
            Self::StatusSuccess => "status.success",
            Self::StatusWarning => "status.warning",
            Self::StatusError => "status.error",
            Self::StatusReady => "status.ready",
            Self::StatusInfo => "status.info",
            Self::SurfaceBottomPaneBg => "surface.bottom_pane.bg",
            Self::BadgeFgDark => "badge.fg.dark",
            Self::InteractionSubAgent => "interaction.sub_agent",
            Self::PendingCardFg => "pending.card.fg",
            Self::ToolStderrFg => "tool.stderr.fg",
            Self::DiffAddBg => "diff.add.bg",
            Self::DiffAddFg => "diff.add.fg",
            Self::DiffDelBg => "diff.del.bg",
            Self::DiffDelFg => "diff.del.fg",
            Self::DiffHunkBg => "diff.hunk.bg",
            Self::DiffHunkFg => "diff.hunk.fg",
            Self::DiffContextFg => "diff.context.fg",
            Self::MarkdownHeading => "markdown.heading",
            Self::MarkdownLink => "markdown.link",
            Self::MarkdownCode => "markdown.code",
            Self::MarkdownBlockQuote => "markdown.block_quote",
            Self::MarkdownListBullet => "markdown.list_bullet",
            Self::MarkdownBold => "markdown.bold",
            Self::MarkdownItalic => "markdown.italic",
            Self::BudgetSystem => "budget.system",
            Self::BudgetWorkspace => "budget.workspace",
            Self::BudgetActive => "budget.active",
            Self::BudgetHistory => "budget.history",
            Self::BudgetMemory => "budget.memory",
            Self::BudgetOutput => "budget.output",
            Self::BudgetFree => "budget.free",
        }
    }

    fn default_color(self) -> Color {
        match self {
            Self::UiElementBg => UI_ELEMENT_BG,
            Self::PopupBg => POPUP_BG,
            Self::PopupDimmerBg => POPUP_DIMMER_BG,
            Self::TextPrimary => TEXT_PRIMARY,
            Self::TextSecondary => TEXT_SECONDARY,
            Self::TextAccent => TEXT_ACCENT,
            Self::TextMuted => TEXT_MUTED,
            Self::PickerItemFg => PICKER_ITEM_FG,
            Self::PickerItemMutedFg => PICKER_ITEM_MUTED_FG,
            Self::PickerHighlightFg => PICKER_HIGHLIGHT_FG,
            Self::PickerHighlightBg => PICKER_HIGHLIGHT_BG,
            Self::RoleUser => ROLE_USER,
            Self::RoleAgent => ROLE_AGENT,
            Self::RoleSystem => ROLE_SYSTEM,
            Self::RolePrefix => ROLE_PREFIX,
            Self::PhaseExploring => PHASE_EXPLORING,
            Self::PhaseExplored => PHASE_EXPLORED,
            Self::PhasePlanning => PHASE_PLANNING,
            Self::PhaseRunning => PHASE_RUNNING,
            Self::PhaseRan => PHASE_RAN,
            Self::StatusSuccess => STATUS_SUCCESS,
            Self::StatusWarning => STATUS_WARNING,
            Self::StatusError => STATUS_ERROR,
            Self::StatusReady => STATUS_READY,
            Self::StatusInfo => STATUS_INFO,
            Self::SurfaceBottomPaneBg => SURFACE_BOTTOM_PANE_BG,
            Self::BadgeFgDark => BADGE_FG_DARK,
            Self::InteractionSubAgent => INTERACTION_SUB_AGENT,
            Self::PendingCardFg => PENDING_CARD_FG,
            Self::ToolStderrFg => TOOL_STDERR_FG,
            Self::DiffAddBg => DIFF_ADD_BG,
            Self::DiffAddFg => DIFF_ADD_FG,
            Self::DiffDelBg => DIFF_DEL_BG,
            Self::DiffDelFg => DIFF_DEL_FG,
            Self::DiffHunkBg => DIFF_HUNK_BG,
            Self::DiffHunkFg => DIFF_HUNK_FG,
            Self::DiffContextFg => DIFF_CONTEXT_FG,
            Self::MarkdownHeading => MD_HEADING,
            Self::MarkdownLink => MD_LINK,
            Self::MarkdownCode => MD_CODE,
            Self::MarkdownBlockQuote => MD_BLOCK_QUOTE,
            Self::MarkdownListBullet => MD_LIST_BULLET,
            Self::MarkdownBold => MD_BOLD,
            Self::MarkdownItalic => MD_ITALIC,
            Self::BudgetSystem => BUDGET_SYSTEM,
            Self::BudgetWorkspace => BUDGET_WORKSPACE,
            Self::BudgetActive => BUDGET_ACTIVE,
            Self::BudgetHistory => BUDGET_HISTORY,
            Self::BudgetMemory => BUDGET_MEMORY,
            Self::BudgetOutput => BUDGET_OUTPUT,
            Self::BudgetFree => BUDGET_FREE,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ResolvedTuiTheme {
    tokens: BTreeMap<ThemeToken, Color>,
}

impl ResolvedTuiTheme {
    fn from_config(config: &TuiThemeConfig) -> Self {
        let mut tokens = BTreeMap::new();
        for (key, value) in &config.tokens {
            let Some(token) = token_from_key(key) else {
                log::warn!("unknown TUI theme token `{key}`");
                continue;
            };
            let Some(color) = parse_color_value(value) else {
                log::warn!("invalid TUI theme color `{value}` for token `{key}`");
                continue;
            };
            tokens.insert(token, color);
        }
        Self { tokens }
    }

    fn color(&self, token: ThemeToken) -> Color {
        self.tokens
            .get(&token)
            .copied()
            .unwrap_or_else(|| token.default_color())
    }
}

static ACTIVE_THEME: OnceLock<RwLock<ResolvedTuiTheme>> = OnceLock::new();

fn theme_lock() -> &'static RwLock<ResolvedTuiTheme> {
    ACTIVE_THEME.get_or_init(|| RwLock::new(ResolvedTuiTheme::default()))
}

pub(crate) fn install_config(config: &TuiThemeConfig) {
    let theme = ResolvedTuiTheme::from_config(config);
    match theme_lock().write() {
        Ok(mut active) => *active = theme,
        Err(poisoned) => *poisoned.into_inner() = theme,
    }
    crate::tui::highlight::install_syntax_theme(config.syntax_theme.as_deref());
}

pub(crate) fn theme_color(token: ThemeToken) -> Color {
    match theme_lock().read() {
        Ok(active) => active.color(token),
        Err(poisoned) => poisoned.into_inner().color(token),
    }
}

pub(crate) fn parse_color_value(value: &str) -> Option<Color> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("reset") {
        return Some(Color::Reset);
    }
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex_color(hex);
    }
    if let Some(index) = value.strip_prefix("ansi:") {
        return index.parse::<u8>().ok().map(Color::Indexed);
    }
    color_name(value)
}

fn parse_hex_color(hex: &str) -> Option<Color> {
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

fn color_name(value: &str) -> Option<Color> {
    match value.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "dark_gray" | "dark_grey" | "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "light_red" | "lightred" => Some(Color::LightRed),
        "light_green" | "lightgreen" => Some(Color::LightGreen),
        "light_yellow" | "lightyellow" => Some(Color::LightYellow),
        "light_blue" | "lightblue" => Some(Color::LightBlue),
        "light_magenta" | "lightmagenta" => Some(Color::LightMagenta),
        "light_cyan" | "lightcyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

fn token_from_key(key: &str) -> Option<ThemeToken> {
    let normalized = key.trim().replace('-', ".").to_ascii_lowercase();
    THEME_TOKENS
        .iter()
        .copied()
        .find(|token| token.key() == normalized)
}

// See https://www.nordtheme.com/docs/colors-and-palettes
pub(crate) const NORD0: Color = Color::Rgb(0x2E, 0x34, 0x40); // Polar Night (darkest bg)
pub(crate) const NORD1: Color = Color::Rgb(0x3B, 0x42, 0x52); // Polar Night
pub(crate) const NORD2: Color = Color::Rgb(0x43, 0x4C, 0x5E); // Polar Night
pub(crate) const NORD3: Color = Color::Rgb(0x4C, 0x56, 0x6A); // Polar Night (lightest bg)
pub(crate) const NORD4: Color = Color::Rgb(0xD8, 0xDE, 0xE9); // Snow Storm (darkest fg)
pub(crate) const NORD6: Color = Color::Rgb(0xEC, 0xEF, 0xF4); // Snow Storm (brightest fg)
pub(crate) const NORD7: Color = Color::Rgb(0x8F, 0xBC, 0xBB); // Frost (green-cyan)
pub(crate) const NORD8: Color = Color::Rgb(0x88, 0xC0, 0xD0); // Frost (cyan)
pub(crate) const NORD9: Color = Color::Rgb(0x81, 0xA1, 0xC1); // Frost (blue-gray)
pub(crate) const NORD10: Color = Color::Rgb(0x5E, 0x81, 0xAC); // Frost (dark blue)
pub(crate) const NORD11: Color = Color::Rgb(0xBF, 0x61, 0x6A); // Aurora (red)
pub(crate) const NORD12: Color = Color::Rgb(0xD0, 0x87, 0x70); // Aurora (orange)
pub(crate) const NORD13: Color = Color::Rgb(0xEB, 0xCB, 0x8B); // Aurora (yellow)
pub(crate) const NORD14: Color = Color::Rgb(0xA3, 0xBE, 0x8C); // Aurora (green)
pub(crate) const NORD15: Color = Color::Rgb(0xB4, 0x8E, 0xAD); // Aurora (purple)

// ── UI surface colors ───────────────────────────────────────────
pub(crate) const UI_ELEMENT_BG: Color = NORD2;

// ── Popup / overlay surfaces ────────────────────────────────────
pub(crate) const POPUP_BG: Color = NORD1;
/// Full-screen dimmer behind centered popups.
pub(crate) const POPUP_DIMMER_BG: Color = NORD0;

// ── Text ────────────────────────────────────────────────────────
pub(crate) const TEXT_PRIMARY: Color = NORD4;
pub(crate) const TEXT_SECONDARY: Color = NORD3;
pub(crate) const TEXT_ACCENT: Color = NORD8;
pub(crate) const TEXT_MUTED: Color = NORD2;

// ── Picker overlays ────────────────────────────────────────────
pub(crate) const PICKER_ITEM_FG: Color = NORD4;
pub(crate) const PICKER_ITEM_MUTED_FG: Color = NORD3;
pub(crate) const PICKER_HIGHLIGHT_FG: Color = NORD6;
pub(crate) const PICKER_HIGHLIGHT_BG: Color = NORD10;

// ── Message roles ───────────────────────────────────────────────
pub(crate) const ROLE_USER: Color = NORD9;
pub(crate) const ROLE_AGENT: Color = NORD8;
pub(crate) const ROLE_SYSTEM: Color = NORD3;
pub(crate) const ROLE_PREFIX: Color = NORD8;

// ── Progress phases ─────────────────────────────────────────────
pub(crate) const PHASE_EXPLORING: Color = NORD13;
pub(crate) const PHASE_EXPLORED: Color = NORD13;
pub(crate) const PHASE_PLANNING: Color = NORD8;
pub(crate) const PHASE_RUNNING: Color = NORD13;
pub(crate) const PHASE_RAN: Color = NORD7;

// ── Status ──────────────────────────────────────────────────────
pub(crate) const STATUS_SUCCESS: Color = NORD14;
pub(crate) const STATUS_WARNING: Color = NORD13;
pub(crate) const STATUS_ERROR: Color = NORD11;
pub(crate) const STATUS_READY: Color = NORD14;
pub(crate) const STATUS_INFO: Color = NORD9;

// ── UI surfaces (legacy, use UI_* going forward) ────────────────
pub(crate) const SURFACE_BOTTOM_PANE_BG: Color = Color::Reset;

// ── Badge / section label ───────────────────────────────────────
pub(crate) const BADGE_FG_DARK: Color = NORD6;

// ── Interaction ─────────────────────────────────────────────────
pub(crate) const INTERACTION_SUB_AGENT: Color = NORD13;

// ── Pending interaction card background ───────────────────────────
pub(crate) const PENDING_CARD_FG: Color = NORD4;

// ── Tool output ─────────────────────────────────────────────────
pub(crate) const TOOL_STDERR_FG: Color = NORD6;

// ── Diff view ───────────────────────────────────────────────────
pub(crate) const DIFF_ADD_BG: Color = Color::Rgb(21, 58, 42);
pub(crate) const DIFF_ADD_FG: Color = NORD14;
pub(crate) const DIFF_DEL_BG: Color = Color::Rgb(58, 26, 26);
pub(crate) const DIFF_DEL_FG: Color = NORD11;
pub(crate) const DIFF_HUNK_BG: Color = NORD1;
pub(crate) const DIFF_HUNK_FG: Color = NORD3;
pub(crate) const DIFF_CONTEXT_FG: Color = NORD4;

// ── Markdown ────────────────────────────────────────────────────
pub(crate) const MD_HEADING: Color = NORD8;
pub(crate) const MD_LINK: Color = NORD9;
pub(crate) const MD_CODE: Color = NORD14;
pub(crate) const MD_BLOCK_QUOTE: Color = NORD3;
pub(crate) const MD_LIST_BULLET: Color = NORD8;
pub(crate) const MD_BOLD: Color = NORD6;
pub(crate) const MD_ITALIC: Color = NORD4;

// ── Budget bar segments ─────────────────────────────────────────
pub(crate) const BUDGET_SYSTEM: Color = NORD9;
pub(crate) const BUDGET_WORKSPACE: Color = NORD8;
pub(crate) const BUDGET_ACTIVE: Color = NORD14;
pub(crate) const BUDGET_HISTORY: Color = NORD13;
pub(crate) const BUDGET_MEMORY: Color = NORD15;
pub(crate) const BUDGET_OUTPUT: Color = NORD3;
pub(crate) const BUDGET_FREE: Color = NORD2;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn parses_supported_theme_color_values() {
        assert_eq!(
            parse_color_value("#88c0d0"),
            Some(Color::Rgb(0x88, 0xc0, 0xd0))
        );
        assert_eq!(parse_color_value("ansi:12"), Some(Color::Indexed(12)));
        assert_eq!(parse_color_value("reset"), Some(Color::Reset));
        assert_eq!(parse_color_value("light_blue"), Some(Color::LightBlue));
        assert_eq!(parse_color_value("not-a-color"), None);
    }

    #[test]
    fn resolves_configured_token_overrides() {
        let mut tokens = BTreeMap::new();
        tokens.insert("text.accent".to_string(), "#112233".to_string());
        tokens.insert("surface.bottom_pane.bg".to_string(), "reset".to_string());
        tokens.insert("unknown.token".to_string(), "#ffffff".to_string());
        tokens.insert("text-muted".to_string(), "ansi:8".to_string());
        tokens.insert("text.primary".to_string(), "invalid".to_string());
        let config = TuiThemeConfig {
            name: "test".to_string(),
            syntax_theme: None,
            tokens,
        };

        let theme = ResolvedTuiTheme::from_config(&config);

        assert_eq!(
            theme.color(ThemeToken::TextAccent),
            Color::Rgb(0x11, 0x22, 0x33)
        );
        assert_eq!(theme.color(ThemeToken::SurfaceBottomPaneBg), Color::Reset);
        assert_eq!(theme.color(ThemeToken::TextMuted), Color::Indexed(8));
        assert_eq!(theme.color(ThemeToken::TextPrimary), TEXT_PRIMARY);
    }
}
