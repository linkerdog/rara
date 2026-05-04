use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::tui::render::display_width;
use crate::tui::theme::*;

const MAX_DIFF_LINES: usize = 80;

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
}

struct DiffLine {
    kind: DiffLineType,
    text: String,
}

pub(crate) fn render_patch_preview(patch: &str, width: u16) -> Vec<Line<'static>> {
    let files = collect_patch_files(patch);
    if files.is_empty() {
        return render_raw_patch_preview(patch, width);
    }

    let content_width = usize::from(width).saturating_sub(6).max(20);
    let mut lines = Vec::new();
    lines.push(render_summary_header(&files));

    let file_count = files.len();
    let mut emitted = 0usize;
    for (idx, file) in files.iter().enumerate() {
        if idx > 0 {
            lines.push(Line::from(""));
        }

        if file_count > 1 {
            lines.push(render_file_header(file));
        }

        if file.lines.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    "(no inline diff preview)",
                    Style::default().fg(TEXT_SECONDARY),
                ),
            ]));
            continue;
        }

        for diff_line in &file.lines {
            if emitted >= MAX_DIFF_LINES {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        format!(
                            "... {} more diff line(s)",
                            remaining_diff_lines(&files, emitted)
                        ),
                        Style::default().fg(TEXT_SECONDARY),
                    ),
                ]));
                return lines;
            }
            lines.extend(push_wrapped_diff_line(
                diff_line.kind,
                &diff_line.text,
                content_width,
                "    ",
            ));
            emitted += 1;
        }
    }

    lines
}

fn render_raw_patch_preview(patch: &str, width: u16) -> Vec<Line<'static>> {
    let content_width = usize::from(width).saturating_sub(2).max(20);
    let mut lines = Vec::new();
    let mut emitted = 0usize;

    for raw in patch.lines() {
        if matches!(raw, "*** Begin Patch" | "*** End Patch") {
            continue;
        }
        if emitted >= MAX_DIFF_LINES {
            lines.push(Line::from(Span::styled(
                format!(
                    "  ... {} more diff line(s)",
                    patch.lines().count().saturating_sub(emitted)
                ),
                Style::default().fg(TEXT_SECONDARY),
            )));
            break;
        }

        let (kind, text) = classify_patch_line(raw);
        lines.extend(push_wrapped_diff_line(kind, text, content_width, "  "));
        emitted += 1;
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
    }
}

fn push_current_file(files: &mut Vec<DiffFile>, current: &mut Option<DiffFile>) {
    if let Some(file) = current.take() {
        files.push(file);
    }
}

fn render_summary_header(files: &[DiffFile]) -> Line<'static> {
    let added: usize = files.iter().map(|file| file.added).sum();
    let removed: usize = files.iter().map(|file| file.removed).sum();
    let mut spans = vec![Span::styled("* ", Style::default().fg(TEXT_SECONDARY))];

    if let [file] = files {
        spans.push(Span::styled(
            match file.kind {
                DiffFileKind::Add => "Added",
                DiffFileKind::Delete => "Deleted",
                DiffFileKind::Update => "Edited",
            },
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.extend(path_spans(file));
        spans.push(Span::raw(" "));
    } else {
        spans.push(Span::styled(
            "Edited",
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
    let mut spans = vec![Span::styled("  - ", Style::default().fg(TEXT_SECONDARY))];
    spans.extend(path_spans(file));
    spans.push(Span::raw(" "));
    spans.extend(line_count_spans(file.added, file.removed));
    Line::from(spans)
}

fn path_spans(file: &DiffFile) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(file.path.clone())];
    if let Some(move_path) = &file.move_path {
        spans.push(Span::raw(format!(" -> {move_path}")));
    }
    spans
}

fn line_count_spans(added: usize, removed: usize) -> Vec<Span<'static>> {
    vec![
        Span::raw("("),
        Span::styled(format!("+{added}"), Style::default().fg(STATUS_SUCCESS)),
        Span::raw(" "),
        Span::styled(format!("-{removed}"), Style::default().fg(STATUS_ERROR)),
        Span::raw(")"),
    ]
}

fn remaining_diff_lines(files: &[DiffFile], emitted: usize) -> usize {
    files
        .iter()
        .map(|file| file.lines.len())
        .sum::<usize>()
        .saturating_sub(emitted)
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
    width: usize,
    indent: &'static str,
) -> Vec<Line<'static>> {
    let (sign, sign_style, content_style) = match kind {
        DiffLineType::Insert => (
            "+",
            Style::default()
                .fg(STATUS_SUCCESS)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(STATUS_SUCCESS),
        ),
        DiffLineType::Delete => (
            "-",
            Style::default()
                .fg(STATUS_ERROR)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(STATUS_ERROR)
                .add_modifier(Modifier::DIM),
        ),
        DiffLineType::Header => (
            " ",
            Style::default().fg(TEXT_SECONDARY),
            Style::default()
                .fg(TEXT_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        DiffLineType::Context => (
            " ",
            Style::default().fg(TEXT_SECONDARY),
            Style::default().fg(TEXT_MUTED),
        ),
    };

    let content_width = width.saturating_sub(3).max(1);
    let chunks = wrap_plain_text(text, content_width);
    chunks
        .into_iter()
        .enumerate()
        .map(|(idx, chunk)| {
            let prefix = if idx == 0 { sign } else { " " };
            Line::from(vec![
                Span::raw(indent),
                Span::styled(prefix.to_string(), sign_style),
                Span::raw(" "),
                Span::styled(chunk, content_style),
            ])
        })
        .collect()
}

fn wrap_plain_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut rows = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut ch_buf = [0u8; 4];
    for ch in text.chars() {
        let ch_width = display_width(ch.encode_utf8(&mut ch_buf)).max(1);
        if current_width > 0 && current_width + ch_width > width {
            rows.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::render_patch_preview;

    #[test]
    fn renders_patch_preview_with_diff_signs() {
        let lines = render_patch_preview(
            "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n context\n*** End Patch",
            80,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(lines.contains("* Edited src/lib.rs (+1 -1)"));
        assert!(lines.contains("- old"));
        assert!(lines.contains("+ new"));
        assert!(lines.contains("  context"));
    }

    #[test]
    fn renders_patch_preview_grouped_by_file() {
        let lines = render_patch_preview(
            "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** Add File: src/new.rs\n+hello\n*** End Patch",
            80,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(lines.contains("* Edited 2 files (+2 -1)"));
        assert!(lines.contains("src/lib.rs (+1 -1)"));
        assert!(lines.contains("src/new.rs (+1 -0)"));
    }
}
