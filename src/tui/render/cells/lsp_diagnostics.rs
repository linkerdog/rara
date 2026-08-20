use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use serde::Deserialize;

use super::HistoryCell;
use crate::tui::theme::{
    STATUS_ERROR, STATUS_INFO, STATUS_SUCCESS, STATUS_WARNING, TEXT_MUTED, TEXT_SECONDARY,
};

const MAX_VISIBLE_DIAGNOSTICS: usize = 4;

pub(crate) struct LspDiagnosticsCell {
    file: String,
    diagnostics: Vec<LspDiagnostic>,
    freshness: Option<LspDiagnosticFreshness>,
    error: Option<String>,
    status: Option<LspStatus>,
}

#[derive(Deserialize)]
struct LspDiagnosticsPayload {
    file: String,
    #[serde(default)]
    diagnostics: Vec<LspDiagnostic>,
    #[serde(default)]
    freshness: Option<LspDiagnosticFreshness>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    status: Option<LspStatus>,
}

#[derive(Deserialize)]
struct LspDiagnostic {
    file: String,
    line: u32,
    column: u32,
    severity: String,
    message: String,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Deserialize)]
struct LspStatus {
    #[serde(default)]
    diagnostic_count: usize,
    #[serde(default)]
    servers: Vec<LspServerStatus>,
}

#[derive(Deserialize)]
struct LspServerStatus {
    #[serde(default)]
    running: bool,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LspDiagnosticFreshness {
    Current,
    Cached,
    Pending,
}

impl LspDiagnosticsCell {
    pub(crate) fn from_message(message: &str) -> Option<Self> {
        let payload = message.trim().strip_prefix("lsp_diagnostics\n")?;
        let payload: LspDiagnosticsPayload = serde_json::from_str(payload.trim()).ok()?;
        Some(Self {
            file: payload.file,
            diagnostics: payload.diagnostics,
            freshness: payload.freshness,
            error: payload.error,
            status: payload.status,
        })
    }
}

impl HistoryCell for LspDiagnosticsCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(vec![Span::styled(
            "LSP Diagnostics",
            Style::default()
                .fg(TEXT_SECONDARY)
                .add_modifier(Modifier::ITALIC),
        )])];

        let status_label = if let Some(error) = self.error.as_deref() {
            format!("error: {error}")
        } else {
            match (self.freshness, self.diagnostics.len()) {
                (Some(LspDiagnosticFreshness::Pending), _) => "diagnostics pending".to_string(),
                (Some(LspDiagnosticFreshness::Cached), 0) => "cached · no diagnostics".to_string(),
                (Some(LspDiagnosticFreshness::Cached), count) => {
                    format!("{count} cached diagnostic(s)")
                }
                (_, 0) => "no diagnostics".to_string(),
                (_, count) => format!("{count} diagnostic(s)"),
            }
        };
        lines.push(Line::from(vec![
            Span::styled(shorten(&self.file, width), Style::default().fg(TEXT_MUTED)),
            Span::raw(" · "),
            Span::styled(status_label, Style::default().fg(self.summary_color())),
        ]));

        for diagnostic in self.diagnostics.iter().take(MAX_VISIBLE_DIAGNOSTICS) {
            lines.push(diagnostic_line(diagnostic, width));
        }

        let hidden = self
            .diagnostics
            .len()
            .saturating_sub(MAX_VISIBLE_DIAGNOSTICS);
        if hidden > 0 {
            lines.push(Line::from(Span::styled(
                format!("... {hidden} more diagnostic(s)"),
                Style::default().fg(TEXT_MUTED),
            )));
        }

        if let Some(status) = self.status.as_ref() {
            lines.push(Line::from(Span::styled(
                status_summary(status),
                Style::default().fg(TEXT_MUTED),
            )));
        }

        lines
    }
}

impl LspDiagnosticsCell {
    fn summary_color(&self) -> ratatui::style::Color {
        if self.error.is_some() {
            STATUS_ERROR
        } else if self.freshness == Some(LspDiagnosticFreshness::Pending) {
            STATUS_INFO
        } else if self.freshness == Some(LspDiagnosticFreshness::Cached) {
            STATUS_WARNING
        } else if self.diagnostics.is_empty() {
            STATUS_SUCCESS
        } else {
            STATUS_WARNING
        }
    }
}

fn diagnostic_line(diagnostic: &LspDiagnostic, width: u16) -> Line<'static> {
    let severity = diagnostic.severity.to_ascii_lowercase();
    let code = diagnostic
        .code
        .as_deref()
        .filter(|code| !code.trim().is_empty())
        .map(|code| format!(" [{code}]"))
        .unwrap_or_default();
    let location = format!(
        "{}:{}:{}",
        diagnostic.file,
        diagnostic.line + 1,
        diagnostic.column + 1
    );
    let prefix = format!("{} {location}{code} ", severity_label(&severity));
    let remaining = usize::from(width)
        .saturating_sub(prefix.chars().count())
        .min(usize::from(width));
    Line::from(vec![
        Span::styled(prefix, Style::default().fg(severity_color(&severity))),
        Span::raw(shorten(&diagnostic.message, remaining as u16)),
    ])
}

fn severity_label(severity: &str) -> &'static str {
    match severity {
        "error" => "[error]",
        "warning" => "[warn]",
        "information" | "info" => "[info]",
        "hint" => "[hint]",
        _ => "[diag]",
    }
}

fn severity_color(severity: &str) -> ratatui::style::Color {
    match severity {
        "error" => STATUS_ERROR,
        "warning" => STATUS_WARNING,
        "information" | "info" | "hint" => STATUS_INFO,
        _ => TEXT_SECONDARY,
    }
}

fn status_summary(status: &LspStatus) -> String {
    let running = status
        .servers
        .iter()
        .filter(|server| server.running)
        .count();
    format!(
        "status: {running} running · {} cached diagnostic(s)",
        status.diagnostic_count
    )
}

fn shorten(value: &str, width: u16) -> String {
    let width = usize::from(width);
    let char_count = value.chars().count();
    if char_count <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width <= 3 {
        return value.chars().take(width).collect();
    }
    let mut collected = value.chars().take(width - 3).collect::<String>();
    collected.push_str("...");
    collected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn renders_lsp_diagnostics_payload() {
        let message = r#"lsp_diagnostics
{
  "file": "src/main.rs",
  "freshness": "current",
  "diagnostics": [{
    "file": "src/main.rs",
    "line": 4,
    "column": 8,
    "severity": "Error",
    "message": "cannot find value `x` in this scope",
    "code": "E0425"
  }],
  "status": {
    "diagnostic_count": 1,
    "servers": [{ "running": true }]
  }
}"#;

        let cell = LspDiagnosticsCell::from_message(message).unwrap();
        let rendered = cell
            .display_lines(120)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("LSP Diagnostics"));
        assert!(rendered.contains("src/main.rs · 1 diagnostic(s)"));
        assert!(rendered.contains("[error] src/main.rs:5:9 [E0425]"));
        assert!(rendered.contains("cannot find value `x`"));
        assert!(rendered.contains("status: 1 running · 1 cached diagnostic(s)"));
    }

    #[test]
    fn renders_lsp_error_payload() {
        let message = r#"lsp_diagnostics
{
  "file": "src/main.rs",
  "diagnostics": [],
  "error": "no LSP server for src/main.rs"
}"#;

        let cell = LspDiagnosticsCell::from_message(message).unwrap();
        let rendered = cell
            .display_lines(120)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("src/main.rs · error: no LSP server for src/main.rs"));
    }

    #[test]
    fn renders_pending_diagnostics_without_claiming_a_clean_file() {
        let message = r#"lsp_diagnostics
{
  "file": "src/main.rs",
  "diagnostics": [],
  "freshness": "pending"
}"#;

        let cell = LspDiagnosticsCell::from_message(message).unwrap();
        let rendered = cell
            .display_lines(120)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("src/main.rs · diagnostics pending"));
        assert!(!rendered.contains("no diagnostics"));
    }

    #[test]
    fn shorten_respects_width_when_truncated() {
        assert_eq!(shorten("abcdef", 0), "");
        assert_eq!(shorten("abcdef", 2), "ab");
        assert_eq!(shorten("abcdef", 3), "abc");
        assert_eq!(shorten("abcdef", 4), "a...");
        assert_eq!(shorten("abcdef", 6), "abcdef");
    }
}
