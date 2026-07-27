//! Pure parser and text engine for RARA's structured `apply_patch` format.

use thiserror::Error;

const PATCH_PREVIEW_LINE_LIMIT: usize = 120;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PatchError {
    #[error("invalid patch: {0}")]
    InvalidInput(String),
    #[error("{0}")]
    ExecutionFailed(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum PatchOp {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        chunks: Vec<Chunk>,
    },
}

impl PatchOp {
    pub fn path(&self) -> &str {
        match self {
            Self::Add { path, .. } | Self::Delete { path } | Self::Update { path, .. } => path,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct PatchAction {
    pub patch: String,
    pub changes: Vec<PatchChange>,
    pub stats: PatchActionStats,
    pub preview: PatchTextPreview,
}

impl PatchAction {
    pub fn summary(&self) -> Vec<String> {
        self.changes
            .iter()
            .map(|change| match change {
                PatchChange::Add { path, .. } => format!("Add file {path}"),
                PatchChange::Delete { path, .. } => format!("Delete file {path}"),
                PatchChange::Update { path, move_to, .. } => format!(
                    "Update file {}{}",
                    path,
                    move_to
                        .as_ref()
                        .map(|target| format!(" -> {target}"))
                        .unwrap_or_default()
                ),
            })
            .collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PatchChange {
    Add {
        path: String,
        content: String,
        lines_added: usize,
    },
    Delete {
        path: String,
        content: String,
        lines_removed: usize,
    },
    Update {
        path: String,
        move_to: Option<String>,
        original_content: String,
        new_content: String,
        stats: PatchStats,
    },
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct PatchActionStats {
    pub files_changed: usize,
    pub hunks_applied: usize,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub created_files: Vec<String>,
    pub deleted_files: Vec<String>,
    pub moved_files: Vec<PatchMove>,
    pub updated_files: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PatchMove {
    pub from: String,
    pub to: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PatchTextPreview {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Chunk {
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Removal,
}

impl DiffLineKind {
    fn from_marker(marker: char) -> Option<Self> {
        match marker {
            ' ' => Some(Self::Context),
            '+' => Some(Self::Addition),
            '-' => Some(Self::Removal),
            _ => None,
        }
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct PatchStats {
    pub hunks_applied: usize,
    pub added_lines: usize,
    pub removed_lines: usize,
}

pub fn parse_patch(patch: &str) -> Result<Vec<PatchOp>, PatchError> {
    let lines: Vec<&str> = patch.lines().collect();
    if lines.first().copied() != Some("*** Begin Patch") {
        return Err(PatchError::InvalidInput(
            "Patch must start with *** Begin Patch".into(),
        ));
    }
    if lines.last().copied() != Some("*** End Patch") {
        return Err(PatchError::InvalidInput(
            "Patch must end with *** End Patch".into(),
        ));
    }

    let mut ops = Vec::new();
    let mut index = 1usize;
    while index + 1 < lines.len() {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut add_lines = Vec::new();
            while index < lines.len() && !lines[index].starts_with("*** ") {
                let content = lines[index];
                let Some(text) = content.strip_prefix('+') else {
                    return Err(PatchError::InvalidInput(format!(
                        "Add file entries must start with '+': {content}"
                    )));
                };
                add_lines.push(text.to_string());
                index += 1;
            }
            ops.push(PatchOp::Add {
                path: path.to_string(),
                lines: add_lines,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            ops.push(PatchOp::Delete {
                path: path.to_string(),
            });
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let mut move_to = None;
            if index < lines.len()
                && let Some(target) = lines[index].strip_prefix("*** Move to: ")
            {
                move_to = Some(target.to_string());
                index += 1;
            }

            let mut chunks = Vec::new();
            let mut current_chunk: Option<Chunk> = None;
            while index < lines.len() && !lines[index].starts_with("*** ") {
                let current = lines[index];
                if current.starts_with("@@") {
                    if let Some(chunk) = current_chunk.take() {
                        chunks.push(chunk);
                    }
                    current_chunk = Some(Chunk { lines: Vec::new() });
                } else if current == "*** End of File" {
                } else {
                    let kind = current.chars().next().ok_or_else(|| {
                        PatchError::InvalidInput("Unexpected empty patch line".into())
                    })?;
                    let Some(kind) = DiffLineKind::from_marker(kind) else {
                        return Err(PatchError::InvalidInput(format!(
                            "Unexpected patch line: {current}"
                        )));
                    };
                    let chunk = current_chunk.get_or_insert_with(|| Chunk { lines: Vec::new() });
                    chunk.lines.push(DiffLine {
                        kind,
                        text: current[1..].to_string(),
                    });
                }
                index += 1;
            }
            if let Some(chunk) = current_chunk.take() {
                chunks.push(chunk);
            }
            ops.push(PatchOp::Update {
                path: path.to_string(),
                move_to,
                chunks,
            });
            continue;
        }

        return Err(PatchError::InvalidInput(format!(
            "Unexpected patch directive: {line}"
        )));
    }

    Ok(ops)
}

pub fn validate_patch_update_context(ops: &[PatchOp]) -> Result<(), PatchError> {
    for op in ops {
        if let PatchOp::Update { path, chunks, .. } = op {
            if chunks.is_empty() {
                return Err(PatchError::ExecutionFailed(format!(
                    "Update patch for {path} must include at least one hunk"
                )));
            }
            for chunk in chunks {
                if chunk
                    .lines
                    .iter()
                    .all(|line| line.kind == DiffLineKind::Addition)
                {
                    return Err(PatchError::ExecutionFailed(format!(
                        "Patch hunk for {path} must include at least one context or removed line"
                    )));
                }
            }
        }
    }
    Ok(())
}

pub fn build_patch_action(
    patch: &str,
    mut read_file: impl FnMut(&str) -> Result<Option<String>, PatchError>,
) -> Result<PatchAction, PatchError> {
    let ops = parse_patch(patch)?;
    build_patch_action_from_ops(patch, ops, &mut read_file)
}

pub fn build_patch_action_from_ops(
    patch: &str,
    ops: Vec<PatchOp>,
    read_file: &mut impl FnMut(&str) -> Result<Option<String>, PatchError>,
) -> Result<PatchAction, PatchError> {
    validate_patch_update_context(&ops)?;

    let mut changes = Vec::new();
    let mut stats = PatchActionStats::default();

    for op in ops {
        match op {
            PatchOp::Add { path, lines } => {
                let content = join_lines(&lines);
                stats.files_changed += 1;
                stats.hunks_applied += 1;
                stats.added_lines += lines.len();
                stats.created_files.push(path.clone());
                changes.push(PatchChange::Add {
                    path,
                    content,
                    lines_added: lines.len(),
                });
            }
            PatchOp::Delete { path } => {
                let content = read_file(&path)?.ok_or_else(|| {
                    PatchError::ExecutionFailed(format!("Cannot delete missing file {path}"))
                })?;
                let lines_removed = split_lines(&content).len();
                stats.files_changed += 1;
                stats.hunks_applied += 1;
                stats.removed_lines += lines_removed;
                stats.deleted_files.push(path.clone());
                changes.push(PatchChange::Delete {
                    path,
                    content,
                    lines_removed,
                });
            }
            PatchOp::Update {
                path,
                move_to,
                chunks,
            } => {
                let original_content = read_file(&path)?.ok_or_else(|| {
                    PatchError::ExecutionFailed(format!("Cannot update missing file {path}"))
                })?;
                let mut update_stats = PatchStats::default();
                let new_content =
                    apply_update_chunks(&path, &original_content, &chunks, &mut update_stats)?;

                stats.files_changed += 1;
                stats.hunks_applied += update_stats.hunks_applied;
                stats.added_lines += update_stats.added_lines;
                stats.removed_lines += update_stats.removed_lines;
                stats.updated_files.push(path.clone());
                if let Some(target) = &move_to {
                    stats.moved_files.push(PatchMove {
                        from: path.clone(),
                        to: target.clone(),
                    });
                }
                changes.push(PatchChange::Update {
                    path,
                    move_to,
                    original_content,
                    new_content,
                    stats: update_stats,
                });
            }
        }
    }

    let (text, truncated) = patch_preview(patch);
    Ok(PatchAction {
        patch: patch.to_string(),
        changes,
        stats,
        preview: PatchTextPreview { text, truncated },
    })
}

pub fn apply_update_chunks(
    path: &str,
    original: &str,
    chunks: &[Chunk],
    stats: &mut PatchStats,
) -> Result<String, PatchError> {
    let original_lines = split_lines(original);

    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut cursor = 0usize;

    for chunk in chunks {
        let mut old_lines = Vec::new();
        let mut added_in_chunk = 0usize;
        let mut removed_in_chunk = 0usize;

        let mut new_lines: Vec<String> = Vec::new();
        for line in &chunk.lines {
            match line.kind {
                DiffLineKind::Context => {
                    old_lines.push(line.text.clone());
                    new_lines.push(line.text.clone());
                }
                DiffLineKind::Addition => {
                    new_lines.push(line.text.clone());
                    added_in_chunk += 1;
                }
                DiffLineKind::Removal => {
                    old_lines.push(line.text.clone());
                    removed_in_chunk += 1;
                }
            }
        }

        let Some(pos) = seek_sequence(&original_lines, &old_lines, cursor, false) else {
            return Err(PatchError::ExecutionFailed(format!(
                "Patch hunk did not match file {path}"
            )));
        };

        let actual: &[String] = &original_lines[pos..pos + old_lines.len()];
        let new_lines = if old_lines != actual {
            preserve_file_quote_style(actual, &new_lines)
        } else {
            new_lines
        };

        replacements.push((pos, old_lines.len(), new_lines));
        cursor = pos + old_lines.len();
        stats.hunks_applied += 1;
        stats.added_lines += added_in_chunk;
        stats.removed_lines += removed_in_chunk;
    }

    let mut output = original_lines;
    replacements.sort_by_key(|(pos, _, _)| *pos);
    for (pos, old_len, new_lines) in replacements.into_iter().rev() {
        let end = pos + old_len;
        output.splice(pos..end, new_lines);
    }

    Ok(join_lines(&output))
}

pub fn split_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

pub fn join_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

pub fn patch_preview(patch: &str) -> (String, bool) {
    let lines = patch
        .lines()
        .take(PATCH_PREVIEW_LINE_LIMIT)
        .collect::<Vec<_>>();
    let truncated = patch.lines().nth(PATCH_PREVIEW_LINE_LIMIT).is_some();
    let mut preview = lines.join("\n");
    if truncated {
        preview.push_str("\n... diff truncated");
    }
    (preview, truncated)
}

pub fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    let search_start = if eof && lines.len() >= pattern.len() {
        lines.len() - pattern.len()
    } else {
        start
    };

    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }

    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(a, b)| a.trim_end() == b.trim_end())
        {
            return Some(i);
        }
    }

    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(a, b)| a.trim() == b.trim())
        {
            return Some(i);
        }
    }

    let npattern: Vec<String> = pattern.iter().map(|s| normalise_unicode(s)).collect();
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(&npattern)
            .all(|(a, b)| normalise_unicode(a) == *b)
        {
            return Some(i);
        }
    }

    None
}

pub fn normalise_unicode(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

fn preserve_file_quote_style(actual: &[String], new_lines: &[String]) -> Vec<String> {
    let has_curly_double = actual
        .iter()
        .any(|line| line.contains('\u{201C}') || line.contains('\u{201D}'));
    let has_curly_single = actual
        .iter()
        .any(|line| line.contains('\u{2018}') || line.contains('\u{2019}'));
    if !has_curly_double && !has_curly_single {
        return new_lines.to_vec();
    }

    new_lines
        .iter()
        .map(|line| {
            let mut result = String::with_capacity(line.len());
            let chars: Vec<char> = line.chars().collect();
            for (i, &ch) in chars.iter().enumerate() {
                if has_curly_double && (ch == '"') {
                    result.push(if is_opening_context(&chars, i) {
                        '\u{201C}'
                    } else {
                        '\u{201D}'
                    });
                } else if has_curly_single && (ch == '\'') {
                    let prev_is_letter = i > 0 && chars[i - 1].is_alphabetic();
                    let next_is_letter = i + 1 < chars.len() && chars[i + 1].is_alphabetic();
                    if prev_is_letter && next_is_letter {
                        result.push('\u{2019}');
                    } else {
                        result.push(if is_opening_context(&chars, i) {
                            '\u{2018}'
                        } else {
                            '\u{2019}'
                        });
                    }
                } else {
                    result.push(ch);
                }
            }
            result
        })
        .collect()
}

fn is_opening_context(chars: &[char], pos: usize) -> bool {
    if pos == 0 {
        return true;
    }
    let prev = chars[pos - 1];
    matches!(
        prev,
        ' ' | '\t' | '\n' | '(' | '[' | '{' | '\u{2014}' | '\u{2013}'
    )
}

#[cfg(test)]
mod tests {
    use std::string::ToString;

    use super::{
        PatchChange, PatchError, PatchMove, PatchOp, PatchStats, apply_update_chunks,
        build_patch_action, normalise_unicode, parse_patch, seek_sequence,
        validate_patch_update_context,
    };

    fn to_vec(strings: &[&str]) -> Vec<String> {
        strings.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn parse_patch_returns_typed_operations() {
        let ops = parse_patch(
            "*** Begin Patch\n\
             *** Add File: new.txt\n\
             +hello\n\
             *** Delete File: old.txt\n\
             *** Update File: input.txt\n\
             *** Move to: output.txt\n\
             @@\n\
             -old\n\
             +new\n\
             *** End Patch",
        )
        .expect("patch parses");

        assert!(matches!(ops[0], PatchOp::Add { .. }));
        assert!(matches!(ops[1], PatchOp::Delete { .. }));
        assert!(matches!(
            &ops[2],
            PatchOp::Update {
                path,
                move_to: Some(target),
                chunks
            } if path == "input.txt" && target == "output.txt" && chunks.len() == 1
        ));
    }

    #[test]
    fn validate_patch_rejects_add_only_update_hunks() {
        let ops = parse_patch(
            "*** Begin Patch\n\
             *** Update File: input.txt\n\
             @@\n\
             +inserted\n\
             *** End Patch",
        )
        .expect("patch parses");

        assert_eq!(
            validate_patch_update_context(&ops),
            Err(PatchError::ExecutionFailed(
                "Patch hunk for input.txt must include at least one context or removed line"
                    .to_string()
            ))
        );
    }

    #[test]
    fn apply_update_chunks_preserves_existing_behavior() {
        let ops = parse_patch(
            r#"*** Begin Patch
*** Update File: input.txt
@@
-hello
+hi
 world
*** End Patch"#,
        )
        .expect("patch parses");
        let PatchOp::Update { chunks, .. } = &ops[0] else {
            panic!("expected update");
        };

        let mut stats = PatchStats::default();
        let updated = apply_update_chunks("input.txt", "hello\nworld\n", chunks, &mut stats)
            .expect("patch applies");

        assert_eq!(updated, "hi\nworld\n");
        assert_eq!(
            stats,
            PatchStats {
                hunks_applied: 1,
                added_lines: 1,
                removed_lines: 1,
            }
        );
    }

    #[test]
    fn build_patch_action_previews_changes_and_stats() {
        let action = build_patch_action(
            "*** Begin Patch\n\
             *** Add File: created.txt\n\
             +hello\n\
             *** Delete File: obsolete.txt\n\
             *** Update File: original.txt\n\
             *** Move to: moved.txt\n\
             @@\n\
             -old\n\
             +new\n\
             *** End Patch",
            |path| match path {
                "obsolete.txt" => Ok(Some("gone\n".to_string())),
                "original.txt" => Ok(Some("old\n".to_string())),
                _ => Ok(None),
            },
        )
        .expect("action builds");

        assert_eq!(action.changes.len(), 3);
        assert_eq!(action.stats.files_changed, 3);
        assert_eq!(action.stats.hunks_applied, 3);
        assert_eq!(action.stats.added_lines, 2);
        assert_eq!(action.stats.removed_lines, 2);
        assert_eq!(action.stats.created_files, vec!["created.txt"]);
        assert_eq!(action.stats.deleted_files, vec!["obsolete.txt"]);
        assert_eq!(action.stats.updated_files, vec!["original.txt"]);
        assert_eq!(
            action.stats.moved_files,
            vec![PatchMove {
                from: "original.txt".to_string(),
                to: "moved.txt".to_string(),
            }]
        );
        assert_eq!(
            action.summary(),
            vec![
                "Add file created.txt",
                "Delete file obsolete.txt",
                "Update file original.txt -> moved.txt",
            ]
        );
        assert!(matches!(
            &action.changes[2],
            PatchChange::Update {
                original_content,
                new_content,
                ..
            } if original_content == "old\n" && new_content == "new\n"
        ));
    }

    #[test]
    fn build_patch_action_rejects_missing_update_target() {
        let error = build_patch_action(
            "*** Begin Patch\n\
             *** Update File: missing.txt\n\
             @@\n\
             -old\n\
             +new\n\
             *** End Patch",
            |_| Ok(None),
        )
        .expect_err("missing update target should fail");

        assert_eq!(
            error,
            PatchError::ExecutionFailed("Cannot update missing file missing.txt".to_string())
        );
    }

    #[test]
    fn exact_match_finds_sequence() {
        let lines = to_vec(&["foo", "bar", "baz"]);
        let pattern = to_vec(&["bar", "baz"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
    }

    #[test]
    fn exact_match_respects_start() {
        let lines = to_vec(&["foo", "bar", "baz", "qux"]);
        let pattern = to_vec(&["baz", "qux"]);
        assert_eq!(seek_sequence(&lines, &pattern, 2, false), Some(2));
        assert_eq!(seek_sequence(&lines, &pattern, 3, false), None);
    }

    #[test]
    fn rstrip_match_ignores_trailing_whitespace() {
        let lines = to_vec(&["foo ", "bar\t\t"]);
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn trim_match_ignores_leading_whitespace() {
        let lines = to_vec(&["  foo", "\tbar"]);
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn unicode_normalised_match_curly_double_quotes() {
        let lines = to_vec(&["\u{201C}hello\u{201D}"]);
        let pattern = to_vec(&["\"hello\""]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn unicode_normalised_match_curly_single_quotes() {
        let lines = to_vec(&["\u{2018}world\u{2019}"]);
        let pattern = to_vec(&["'world'"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn unicode_normalised_match_em_dash() {
        let lines = to_vec(&["before\u{2014}after"]);
        let pattern = to_vec(&["before-after"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn unicode_normalised_match_nbsp() {
        let lines = to_vec(&["hello\u{00A0}world"]);
        let pattern = to_vec(&["hello world"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn empty_pattern_returns_start() {
        let lines = to_vec(&["a", "b"]);
        assert_eq!(seek_sequence(&lines, &[], 0, false), Some(0));
        assert_eq!(seek_sequence(&lines, &[], 1, false), Some(1));
    }

    #[test]
    fn pattern_longer_than_lines_returns_none() {
        let lines = to_vec(&["a"]);
        let pattern = to_vec(&["a", "b"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), None);
    }

    #[test]
    fn not_found_returns_none() {
        let lines = to_vec(&["foo", "bar"]);
        let pattern = to_vec(&["baz"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), None);
    }

    #[test]
    fn eof_flag_starts_at_end() {
        let lines = to_vec(&["a", "b", "c", "b", "c"]);
        let pattern = to_vec(&["b", "c"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
        assert_eq!(seek_sequence(&lines, &pattern, 0, true), Some(3));
    }

    #[test]
    fn normalise_unicode_handles_en_dash() {
        assert_eq!(normalise_unicode("\u{2013}"), "-");
    }

    #[test]
    fn normalise_unicode_handles_figure_dash() {
        assert_eq!(normalise_unicode("\u{2012}"), "-");
    }

    #[test]
    fn normalise_unicode_handles_minus_sign() {
        assert_eq!(normalise_unicode("\u{2212}"), "-");
    }

    #[test]
    fn normalise_unicode_handles_thin_space() {
        assert_eq!(normalise_unicode("\u{2009}"), "");
    }
}
