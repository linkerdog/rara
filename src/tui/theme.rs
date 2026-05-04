// Semantic color tokens for the TUI — keep this file small and current.
//
// All render-layer code should use these constants instead of raw
// ratatui::style::Color values so that the color palette can be
// changed in one place and the visual hierarchy stays consistent.
use ratatui::style::Color;

// ── Text ────────────────────────────────────────────────────────
pub(crate) const TEXT_SECONDARY: Color = Color::DarkGray;
pub(crate) const TEXT_ACCENT: Color = Color::Cyan;
pub(crate) const TEXT_MUTED: Color = Color::Gray;

// ── Message roles ───────────────────────────────────────────────
pub(crate) const ROLE_USER: Color = Color::LightBlue;
pub(crate) const ROLE_AGENT: Color = Color::Cyan;
pub(crate) const ROLE_SYSTEM: Color = Color::Gray;
pub(crate) const ROLE_PREFIX: Color = Color::Cyan;

// ── Progress phases ─────────────────────────────────────────────
pub(crate) const PHASE_EXPLORING: Color = Color::Yellow;
pub(crate) const PHASE_EXPLORED: Color = Color::Rgb(231, 201, 92);
pub(crate) const PHASE_THINKING: Color = Color::LightBlue;
pub(crate) const PHASE_PLANNING: Color = Color::Cyan;
pub(crate) const PHASE_RUNNING: Color = Color::Yellow;
pub(crate) const PHASE_RAN: Color = Color::LightYellow;

// ── Status ──────────────────────────────────────────────────────
pub(crate) const STATUS_SUCCESS: Color = Color::LightGreen;
pub(crate) const STATUS_WARNING: Color = Color::Yellow;
pub(crate) const STATUS_ERROR: Color = Color::Red;
pub(crate) const STATUS_READY: Color = Color::Green;
pub(crate) const STATUS_INFO: Color = Color::LightBlue;

// ── UI surfaces ─────────────────────────────────────────────────
pub(crate) const SURFACE_BOTTOM_PANE_BG: Color = Color::Reset;

// ── Badge / section label ───────────────────────────────────────
pub(crate) const BADGE_FG_DARK: Color = Color::White;
pub(crate) const BADGE_FG_LIGHT: Color = Color::Black;

// ── Interaction ─────────────────────────────────────────────────
pub(crate) const INTERACTION_SUB_AGENT: Color = Color::Rgb(231, 201, 92); // gold distinct from green RequestInput

// ── Tool output ─────────────────────────────────────────────────
pub(crate) const TOOL_STDERR_BG: Color = Color::Rgb(172, 76, 108);
pub(crate) const TOOL_STDERR_FG: Color = Color::White;

// ── Diff view ───────────────────────────────────────────────────
pub(crate) const DIFF_ADD_BG: Color = Color::Rgb(21, 58, 42);
pub(crate) const DIFF_ADD_FG: Color = Color::Rgb(74, 222, 128);
pub(crate) const DIFF_DEL_BG: Color = Color::Rgb(58, 26, 26);
pub(crate) const DIFF_DEL_FG: Color = Color::Rgb(248, 113, 113);
pub(crate) const DIFF_HUNK_BG: Color = Color::Rgb(30, 30, 46);
pub(crate) const DIFF_HUNK_FG: Color = Color::Rgb(148, 163, 184);

// ── Budget bar segments ─────────────────────────────────────────
pub(crate) const BUDGET_SYSTEM: Color = Color::LightBlue;
pub(crate) const BUDGET_WORKSPACE: Color = Color::LightCyan;
pub(crate) const BUDGET_ACTIVE: Color = Color::LightGreen;
pub(crate) const BUDGET_HISTORY: Color = Color::Rgb(231, 201, 92);
pub(crate) const BUDGET_MEMORY: Color = Color::LightMagenta;
pub(crate) const BUDGET_OUTPUT: Color = Color::Gray;
pub(crate) const BUDGET_FREE: Color = Color::DarkGray;
