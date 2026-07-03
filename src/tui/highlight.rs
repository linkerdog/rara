use std::sync::{OnceLock, RwLock};

use ratatui::{
    style::{Color as RtColor, Modifier, Style},
    text::{Line, Span},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Color as SyntectColor, FontStyle, Style as SyntectStyle, Theme},
    parsing::SyntaxReference,
    util::LinesWithEndings,
};
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

static SYNTAX_SET: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
static THEME: OnceLock<RwLock<Theme>> = OnceLock::new();

const ANSI_ALPHA_INDEX: u8 = 0x00;
const ANSI_ALPHA_DEFAULT: u8 = 0x01;
const OPAQUE_ALPHA: u8 = 0xFF;
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;

fn syntax_set() -> &'static syntect::parsing::SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme_lock() -> &'static RwLock<Theme> {
    THEME.get_or_init(|| RwLock::new(default_syntax_theme()))
}

fn default_syntax_theme() -> Theme {
    two_face::theme::extra()
        .get(EmbeddedThemeName::CatppuccinMocha)
        .clone()
}

pub(crate) fn install_syntax_theme(name: Option<&str>) {
    let theme = name
        .and_then(|name| {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return None;
            }
            EmbeddedLazyThemeSet::theme_names()
                .iter()
                .copied()
                .find(|candidate| candidate.as_name().eq_ignore_ascii_case(trimmed))
                .map(|theme_name| two_face::theme::extra().get(theme_name).clone())
                .or_else(|| {
                    log::warn!("unknown syntax theme `{trimmed}`, using default syntax theme");
                    None
                })
        })
        .unwrap_or_else(default_syntax_theme);
    match theme_lock().write() {
        Ok(mut active) => *active = theme,
        Err(poisoned) => *poisoned.into_inner() = theme,
    }
}

fn current_syntax_theme() -> Theme {
    match theme_lock().read() {
        Ok(theme) => theme.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

#[allow(clippy::disallowed_methods)]
fn ansi_palette_color(index: u8) -> RtColor {
    match index {
        0x00 => RtColor::Black,
        0x01 => RtColor::Red,
        0x02 => RtColor::Green,
        0x03 => RtColor::Yellow,
        0x04 => RtColor::Blue,
        0x05 => RtColor::Magenta,
        0x06 => RtColor::Cyan,
        0x07 => RtColor::Gray,
        n => RtColor::Indexed(n),
    }
}

#[allow(clippy::disallowed_methods)]
fn convert_syntect_color(color: SyntectColor) -> Option<RtColor> {
    match color.a {
        ANSI_ALPHA_INDEX => Some(ansi_palette_color(color.r)),
        ANSI_ALPHA_DEFAULT => None,
        OPAQUE_ALPHA => Some(RtColor::Rgb(color.r, color.g, color.b)),
        _ => Some(RtColor::Rgb(color.r, color.g, color.b)),
    }
}

fn convert_style(syn_style: SyntectStyle) -> Style {
    let mut rt_style = Style::default();

    if let Some(fg) = convert_syntect_color(syn_style.foreground) {
        rt_style = rt_style.fg(fg);
    }

    if syn_style.font_style.contains(FontStyle::BOLD) {
        rt_style.add_modifier |= Modifier::BOLD;
    }

    rt_style
}

fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let ss = syntax_set();

    let patched = match lang {
        "csharp" | "c-sharp" => "c#",
        "golang" => "go",
        "python3" => "python",
        "shell" => "bash",
        _ => lang,
    };

    if let Some(syntax) = ss.find_syntax_by_token(patched) {
        return Some(syntax);
    }
    if let Some(syntax) = ss.find_syntax_by_name(patched) {
        return Some(syntax);
    }
    let lower = patched.to_ascii_lowercase();
    if let Some(syntax) = ss
        .syntaxes()
        .iter()
        .find(|syntax| syntax.name.to_ascii_lowercase() == lower)
    {
        return Some(syntax);
    }
    ss.find_syntax_by_extension(lang)
}

fn highlight_to_line_spans_with_theme(
    code: &str,
    lang: &str,
    theme: &Theme,
) -> Option<Vec<Vec<Span<'static>>>> {
    if code.is_empty() {
        return None;
    }
    if code.len() > MAX_HIGHLIGHT_BYTES || code.lines().count() > MAX_HIGHLIGHT_LINES {
        return None;
    }

    let syntax = find_syntax(lang)?;
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(code) {
        let ranges = highlighter.highlight_line(line, syntax_set()).ok()?;
        let mut spans = Vec::new();
        for (style, text) in ranges {
            let text = text.trim_end_matches(['\n', '\r']);
            if text.is_empty() {
                continue;
            }
            spans.push(Span::styled(text.to_string(), convert_style(style)));
        }
        if spans.is_empty() {
            spans.push(Span::raw(String::new()));
        }
        lines.push(spans);
    }

    Some(lines)
}

fn highlight_to_line_spans(code: &str, lang: &str) -> Option<Vec<Vec<Span<'static>>>> {
    let theme = current_syntax_theme();
    highlight_to_line_spans_with_theme(code, lang, &theme)
}

pub(crate) fn highlight_code_to_lines(code: &str, lang: &str) -> Vec<Line<'static>> {
    if let Some(line_spans) = highlight_to_line_spans(code, lang) {
        line_spans.into_iter().map(Line::from).collect()
    } else {
        let mut result: Vec<Line<'static>> = code
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect();
        if result.is_empty() {
            result.push(Line::from(String::new()));
        }
        result
    }
}
