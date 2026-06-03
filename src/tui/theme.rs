// Semantic color tokens for the TUI.
//
// All render-layer code should use these constants instead of raw
// ratatui::style::Color values so that the color palette can be
// changed in one place and the visual hierarchy stays consistent.
//
// The palette is based on the Nord color scheme (matching OpenCode's
// default dark theme) extended with semantic tokens for UI, Markdown,
// and syntax highlighting.
//
// Many constants are reserved for planned rendering features (Markdown
// syntax highlighting, diff views, phase badges, etc.). The full palette
// is kept here as a single source of truth.

use ratatui::style::Color;

// ── Nord palette (reference) ────────────────────────────────────
// See https://www.nordtheme.com/docs/colors-and-palettes

pub(crate) const NORD0: Color = Color::Rgb(0x2E, 0x34, 0x40); // Polar Night (darkest bg)

pub(crate) const NORD1: Color = Color::Rgb(0x3B, 0x42, 0x52); // Polar Night

pub(crate) const NORD2: Color = Color::Rgb(0x43, 0x4C, 0x5E); // Polar Night

pub(crate) const NORD3: Color = Color::Rgb(0x4C, 0x56, 0x6A); // Polar Night (lightest bg)

pub(crate) const NORD4: Color = Color::Rgb(0xD8, 0xDE, 0xE9); // Snow Storm (darkest fg)

pub(crate) const NORD5: Color = Color::Rgb(0xE5, 0xE9, 0xF0); // Snow Storm

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
pub(crate) const UI_BG: Color = NORD0;
pub(crate) const UI_PANEL_BG: Color = NORD1;
pub(crate) const UI_ELEMENT_BG: Color = NORD2;
pub(crate) const UI_BORDER: Color = NORD3;
pub(crate) const UI_BORDER_ACTIVE: Color = NORD8;

// ── Popup / overlay surfaces ────────────────────────────────────
pub(crate) const POPUP_BG: Color = NORD1;
/// Full-screen dimmer behind centered popups (matches UI_BG).
pub(crate) const POPUP_DIMMER_BG: Color = NORD0;

// ── Text ────────────────────────────────────────────────────────
pub(crate) const TEXT_PRIMARY: Color = NORD4;
pub(crate) const TEXT_SECONDARY: Color = NORD3;
pub(crate) const TEXT_ACCENT: Color = NORD8;
pub(crate) const TEXT_MUTED: Color = NORD2;

// ── Message roles ───────────────────────────────────────────────
pub(crate) const ROLE_USER: Color = NORD9;
pub(crate) const ROLE_AGENT: Color = NORD8;
pub(crate) const ROLE_SYSTEM: Color = NORD3;
pub(crate) const ROLE_PREFIX: Color = NORD8;

// ── Progress phases ─────────────────────────────────────────────
pub(crate) const PHASE_EXPLORING: Color = NORD13;
pub(crate) const PHASE_EXPLORED: Color = NORD13;
pub(crate) const PHASE_THINKING: Color = NORD9;
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
pub(crate) const BADGE_FG_LIGHT: Color = NORD0;

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
pub(crate) const DIFF_CONTEXT_BG: Color = NORD0;
pub(crate) const DIFF_CONTEXT_FG: Color = NORD4;
pub(crate) const DIFF_HIGHLIGHT_BG: Color = NORD2;

// ── Markdown ────────────────────────────────────────────────────
pub(crate) const MD_HEADING: Color = NORD8;
pub(crate) const MD_LINK: Color = NORD9;
pub(crate) const MD_CODE: Color = NORD14;
pub(crate) const MD_CODE_BLOCK: Color = NORD5;
pub(crate) const MD_BLOCK_QUOTE: Color = NORD3;
pub(crate) const MD_LIST_BULLET: Color = NORD8;
pub(crate) const MD_BOLD: Color = NORD6;
pub(crate) const MD_ITALIC: Color = NORD4;

// ── Syntax ──────────────────────────────────────────────────────
pub(crate) const SYNTAX_COMMENT: Color = NORD3;
pub(crate) const SYNTAX_KEYWORD: Color = NORD9;
pub(crate) const SYNTAX_FUNCTION: Color = NORD8;
pub(crate) const SYNTAX_VARIABLE: Color = NORD4;
pub(crate) const SYNTAX_STRING: Color = NORD14;
pub(crate) const SYNTAX_NUMBER: Color = NORD15;
pub(crate) const SYNTAX_TYPE: Color = NORD7;

// ── Budget bar segments ─────────────────────────────────────────
pub(crate) const BUDGET_SYSTEM: Color = NORD9;
pub(crate) const BUDGET_WORKSPACE: Color = NORD8;
pub(crate) const BUDGET_ACTIVE: Color = NORD14;
pub(crate) const BUDGET_HISTORY: Color = NORD13;
pub(crate) const BUDGET_MEMORY: Color = NORD15;
pub(crate) const BUDGET_OUTPUT: Color = NORD3;
pub(crate) const BUDGET_FREE: Color = NORD2;
