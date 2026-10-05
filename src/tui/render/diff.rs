use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::tui::text_wrap::WrapMode;
use crate::tui::theme::{ThemeToken, theme_color, token_fg};
use crate::tui::transcript_text::wrap_line_with_mode;

const MAX_DIFF_LINES_PER_FILE: usize = 80;

#[derive(Clone, Copy)]
enum DiffLineType {
    Insert,
    Delete,
    Context,
    Header,
}

#[derive(Clone, Copy)]
enum DiffFileKind {
    Add,
    Delete,
    Update,
}

struct DiffFile {
    path: String,
    move_path: Option<String>,
    kind: DiffFileKind,
    lines: Vec<DiffLine>,
    added: usize,
    removed: usize,
    total_counts: Option<(usize, usize)>,
    omitted: usize,
}

impl DiffFile {
    fn operation(&self) -> &'static str {
        match self.kind {
            DiffFileKind::Add => "Added",
            DiffFileKind::Delete => "Deleted",
            DiffFileKind::Update if self.move_path.is_some() => "Moved",
            DiffFileKind::Update => "Edited",
        }
    }

    fn counts(&self) -> (usize, Option<usize>) {
        if let Some((added, removed)) = self.total_counts {
            return (added, Some(removed));
        }
        let removed = (!matches!(self.kind, DiffFileKind::Delete) || self.removed > 0)
            .then_some(self.removed);
        (self.added, removed)
    }
}

struct DiffLine {
    kind: DiffLineType,
    text: String,
}

pub(crate) fn render_patch_preview(patch: &str, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let files = collect_patch_files(patch);
    if files.is_empty() {
        return render_raw_patch_preview(patch, width);
    }

    let mut lines = wrap_preview_line(&render_summary_header(&files), width);

    let file_count = files.len();
    // Inventory is independent of hunk budgets, so a large first file cannot
    // hide a later deletion or rename even at the start of the preview.
    if file_count > 1 {
        for file in &files {
            lines.extend(wrap_preview_line(&render_file_header(file), width));
        }
    }
    for (idx, file) in files.iter().enumerate() {
        if idx > 0 || file_count > 1 {
            lines.push(Line::from(""));
        }

        if file_count > 1 {
            lines.extend(wrap_preview_line(&render_file_header(file), width));
        }

        if file.lines.is_empty() {
            lines.extend(wrap_preview_line(
                &Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        "(no inline diff preview)",
                        token_fg(ThemeToken::TextSecondary),
                    ),
                ]),
                width,
            ));
        }

        for diff_line in file.lines.iter().take(MAX_DIFF_LINES_PER_FILE) {
            lines.extend(push_wrapped_diff_line(
                diff_line.kind,
                &diff_line.text,
                width,
                "    ",
            ));
        }
        let omitted = file
            .omitted
            .saturating_add(file.lines.len().saturating_sub(MAX_DIFF_LINES_PER_FILE));
        if omitted > 0 {
            lines.extend(wrap_preview_line(
                &Line::from(Span::styled(
                    format!("    ... {omitted} more diff line(s)"),
                    token_fg(ThemeToken::TextSecondary),
                )),
                width,
            ));
        }
    }

    lines
}

pub(crate) fn render_message_diff_preview(
    role: Option<&str>,
    message: &str,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    let split = split_message_diff(message)?;
    if width == 0 {
        return Some(Vec::new());
    }
    let mut lines = Vec::new();

    if let Some(role) = role.filter(|role| !role.is_empty()) {
        lines.push(Line::from(vec![Span::styled(
            role.to_string(),
            token_fg(ThemeToken::TextSecondary).add_modifier(Modifier::ITALIC),
        )]));
    }

    for line in split.prefix_lines {
        if line.trim().is_empty() {
            continue;
        }
        lines.push(Line::from(format!("  {line}")));
    }

    if split.had_diff_label {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "diff:",
                token_fg(ThemeToken::PhasePlanning).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    lines = lines
        .iter()
        .flat_map(|line| wrap_preview_line(line, width))
        .collect();
    lines.extend(render_patch_preview(&split.patch, width));
    Some(lines)
}

struct MessageDiff {
    prefix_lines: Vec<String>,
    patch: String,
    had_diff_label: bool,
}

fn split_message_diff(message: &str) -> Option<MessageDiff> {
    let mut prefix_lines = Vec::new();
    let mut patch_lines = Vec::new();
    let mut found_diff_label = false;

    for line in message.lines() {
        if !found_diff_label && line.trim_start() == "diff:" {
            found_diff_label = true;
            continue;
        }
        if found_diff_label {
            patch_lines.push(line);
        } else {
            prefix_lines.push(line.to_string());
        }
    }

    if found_diff_label {
        let indent = patch_lines
            .iter()
            .find(|line| !line.trim().is_empty())
            .map_or("", |line| &line[..line.len() - line.trim_start().len()]);
        let patch = patch_lines
            .iter()
            .map(|line| line.strip_prefix(indent).unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        if patch.trim().is_empty() {
            return None;
        }
        return Some(MessageDiff {
            prefix_lines,
            patch,
            had_diff_label: true,
        });
    }

    if message.contains("*** Begin Patch") {
        return Some(MessageDiff {
            prefix_lines: Vec::new(),
            patch: message.to_string(),
            had_diff_label: false,
        });
    }

    None
}

fn render_raw_patch_preview(patch: &str, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let source = patch
        .lines()
        .filter(|raw| !matches!(*raw, "*** Begin Patch" | "*** End Patch"));
    let omitted = source
        .clone()
        .count()
        .saturating_sub(MAX_DIFF_LINES_PER_FILE);
    for raw in source.take(MAX_DIFF_LINES_PER_FILE) {
        let (kind, text) = classify_patch_line(raw);
        lines.extend(push_wrapped_diff_line(kind, text, width, "  "));
    }
    if omitted > 0 {
        lines.extend(wrap_preview_line(
            &Line::from(Span::styled(
                format!("  ... {omitted} more diff line(s)"),
                token_fg(ThemeToken::TextSecondary),
            )),
            width,
        ));
    }

    lines
}

fn collect_patch_files(patch: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current: Option<DiffFile> = None;

    for raw in patch.lines() {
        if matches!(raw, "*** Begin Patch" | "*** End Patch") {
            continue;
        }

        if let Some(path) = raw.strip_prefix("*** Add File: ") {
            push_current_file(&mut files, &mut current);
            current = Some(new_diff_file(path, DiffFileKind::Add));
            continue;
        }

        if let Some(path) = raw.strip_prefix("*** Delete File: ") {
            push_current_file(&mut files, &mut current);
            current = Some(new_diff_file(path, DiffFileKind::Delete));
            continue;
        }

        if let Some(path) = raw.strip_prefix("*** Update File: ") {
            push_current_file(&mut files, &mut current);
            current = Some(new_diff_file(path, DiffFileKind::Update));
            continue;
        }

        let Some(file) = current.as_mut() else {
            continue;
        };

        if let Some(path) = raw.strip_prefix("*** Move to: ") {
            file.move_path = Some(path.to_string());
            continue;
        }

        if let Some(counts) = raw
            .strip_prefix("*** Preview Stats: +")
            .and_then(|value| value.split_once(" -"))
            .and_then(|(added, removed)| Some((added.parse().ok()?, removed.parse().ok()?)))
        {
            file.total_counts = Some(counts);
            continue;
        }
        if let Some(omitted) = raw
            .strip_prefix("*** Preview Omitted: ")
            .and_then(|value| value.parse::<usize>().ok())
        {
            file.omitted = file.omitted.saturating_add(omitted);
            continue;
        }
        if raw == "*** End of File" {
            continue;
        }

        let (kind, text) = classify_patch_line(raw);
        match kind {
            DiffLineType::Insert => file.added += 1,
            DiffLineType::Delete => file.removed += 1,
            DiffLineType::Context | DiffLineType::Header => {}
        }
        file.lines.push(DiffLine {
            kind,
            text: text.to_string(),
        });
    }

    push_current_file(&mut files, &mut current);
    files
}

fn new_diff_file(path: &str, kind: DiffFileKind) -> DiffFile {
    DiffFile {
        path: path.to_string(),
        move_path: None,
        kind,
        lines: Vec::new(),
        added: 0,
        removed: 0,
        total_counts: None,
        omitted: 0,
    }
}

fn push_current_file(files: &mut Vec<DiffFile>, current: &mut Option<DiffFile>) {
    if let Some(file) = current.take() {
        files.push(file);
    }
}

// File paths can contain spaces; word wrapping would discard those spaces
// at a row boundary and change the path shown or copied by the user.
fn wrap_preview_line(line: &Line<'_>, width: u16) -> Vec<Line<'static>> {
    wrap_line_with_mode(line, width, WrapMode::Grapheme)
}

fn render_summary_header(files: &[DiffFile]) -> Line<'static> {
    let added = files
        .iter()
        .fold(0usize, |total, file| total.saturating_add(file.counts().0));
    let removed = files.iter().try_fold(0usize, |total, file| {
        file.counts().1.map(|count| total.saturating_add(count))
    });
    let mut spans = vec![Span::styled("* ", token_fg(ThemeToken::TextSecondary))];

    if let [file] = files {
        spans.push(Span::styled(
            file.operation(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.extend(path_spans(file));
        spans.push(Span::raw(" "));
    } else {
        spans.push(Span::styled(
            "Changed",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(format!(
            " {} {} ",
            files.len(),
            if files.len() == 1 { "file" } else { "files" }
        )));
    }

    spans.extend(line_count_spans(added, removed));
    Line::from(spans)
}

fn render_file_header(file: &DiffFile) -> Line<'static> {
    let mut spans = vec![Span::styled("  - ", token_fg(ThemeToken::TextSecondary))];
    spans.push(Span::styled(
        file.operation(),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.extend(path_spans(file));
    spans.push(Span::raw(" "));
    let (added, removed) = file.counts();
    spans.extend(line_count_spans(added, removed));
    Line::from(spans)
}

fn path_spans(file: &DiffFile) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(file.path.clone())];
    if let Some(move_path) = &file.move_path {
        spans.push(Span::raw(format!(" -> {move_path}")));
    }
    spans
}

fn line_count_spans(added: usize, removed: Option<usize>) -> Vec<Span<'static>> {
    vec![
        Span::raw("("),
        Span::styled(format!("+{added}"), token_fg(ThemeToken::StatusSuccess)),
        Span::raw(" "),
        Span::styled(
            format!(
                "-{}",
                removed.map_or_else(|| "?".to_string(), |count| count.to_string())
            ),
            token_fg(ThemeToken::StatusError),
        ),
        Span::raw(")"),
    ]
}

fn classify_patch_line(line: &str) -> (DiffLineType, &str) {
    if line.starts_with("*** ") || line.starts_with("@@") {
        return (DiffLineType::Header, line);
    }
    if let Some(rest) = line.strip_prefix('+') {
        return (DiffLineType::Insert, rest);
    }
    if let Some(rest) = line.strip_prefix('-') {
        return (DiffLineType::Delete, rest);
    }
    if let Some(rest) = line.strip_prefix(' ') {
        return (DiffLineType::Context, rest);
    }
    (DiffLineType::Context, line)
}

fn push_wrapped_diff_line(
    kind: DiffLineType,
    text: &str,
    width: u16,
    indent: &'static str,
) -> Vec<Line<'static>> {
    let (sign, sign_style, line_bg, content_style) = match kind {
        DiffLineType::Insert => (
            "+",
            token_fg(ThemeToken::DiffAddFg).add_modifier(Modifier::BOLD),
            Some(theme_color(ThemeToken::DiffAddBg)),
            token_fg(ThemeToken::DiffAddFg),
        ),
        DiffLineType::Delete => (
            "-",
            token_fg(ThemeToken::DiffDelFg).add_modifier(Modifier::BOLD),
            Some(theme_color(ThemeToken::DiffDelBg)),
            token_fg(ThemeToken::DiffDelFg).add_modifier(Modifier::DIM),
        ),
        DiffLineType::Header => (
            " ",
            token_fg(ThemeToken::TextSecondary),
            Some(theme_color(ThemeToken::DiffHunkBg)),
            token_fg(ThemeToken::DiffHunkFg).add_modifier(Modifier::BOLD),
        ),
        DiffLineType::Context => (
            " ",
            token_fg(ThemeToken::DiffContextFg),
            None,
            token_fg(ThemeToken::DiffContextFg),
        ),
    };

    let indent = &indent[..indent.len().min(usize::from(width.saturating_sub(4)))];
    let show_sign = width >= 2;
    let separator = if width >= 3 { " " } else { "" };
    let prefix_width = indent.len() + usize::from(show_sign) + separator.len();
    let content_width = width.saturating_sub(prefix_width as u16).max(1);
    wrap_line_with_mode(
        &Line::from(Span::styled(text.to_string(), content_style)),
        content_width,
        WrapMode::Grapheme,
    )
    .into_iter()
    .enumerate()
    .map(|(idx, chunk)| {
        let mut spans = vec![Span::raw(indent.to_string())];
        if show_sign {
            let prefix = if idx == 0 { sign } else { " " };
            spans.push(Span::styled(prefix, sign_style));
        }
        spans.push(Span::raw(separator));
        spans.extend(chunk.spans);
        if let Some(bg) = line_bg {
            for span in &mut spans {
                span.style = span.style.bg(bg);
            }
        }
        Line::from(spans)
    })
    .collect()
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
