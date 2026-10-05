use super::PatchChange;

const PREVIEW_LINES_PER_FILE: usize = 120;

struct Counts {
    added: usize,
    removed: usize,
}

struct FilePreview<'a> {
    header: &'a str,
    move_line: Option<&'a str>,
    end_of_file: bool,
    lines: Vec<&'a str>,
    total_lines: usize,
    observed: Counts,
    validated: Option<Counts>,
}

impl FilePreview<'_> {
    fn finish(self, output: &mut Vec<String>) -> bool {
        output.push(self.header.into());
        if let Some(move_line) = self.move_line {
            output.push(move_line.into());
        }
        // Raw deletion directives have no content to count. Validated actions
        // already read the removed content, so only they can report that total.
        let known = self.validated.is_some()
            || !self.header.starts_with("*** Delete File: ")
            || self.observed.removed > 0;
        if known {
            let counts = self.validated.unwrap_or(self.observed);
            output.push(format!(
                "*** Preview Stats: +{} -{}",
                counts.added, counts.removed
            ));
        }
        output.extend(self.lines.iter().map(|line| (*line).to_owned()));
        let omitted = self.total_lines - self.lines.len();
        if omitted > 0 {
            output.push(format!("*** Preview Omitted: {omitted}"));
        }
        if self.end_of_file {
            output.push("*** End of File".into());
        }
        omitted > 0
    }
}

/// Builds display-only text, retaining every structured file and move directive.
pub fn patch_preview(patch: &str) -> (String, bool) {
    preview_with_counts(patch, std::iter::empty())
}

pub(super) fn action_preview(patch: &str, changes: &[PatchChange]) -> (String, bool) {
    let counts = changes.iter().map(|change| match change {
        PatchChange::Add { lines_added, .. } => Counts {
            added: *lines_added,
            removed: 0,
        },
        PatchChange::Delete { lines_removed, .. } => Counts {
            added: 0,
            removed: *lines_removed,
        },
        PatchChange::Update { stats, .. } => Counts {
            added: stats.added_lines,
            removed: stats.removed_lines,
        },
    });
    preview_with_counts(patch, counts)
}

fn preview_with_counts(patch: &str, mut counts: impl Iterator<Item = Counts>) -> (String, bool) {
    let mut output = Vec::new();
    let mut current: Option<FilePreview<'_>> = None;
    let mut truncated = false;
    let mut unstructured_lines = 0;
    for raw in patch.lines() {
        if raw.starts_with("*** Add File: ")
            || raw.starts_with("*** Delete File: ")
            || raw.starts_with("*** Update File: ")
        {
            if let Some(file) = current.take() {
                truncated |= file.finish(&mut output);
            }
            current = Some(FilePreview {
                header: raw,
                move_line: None,
                end_of_file: false,
                lines: Vec::new(),
                total_lines: 0,
                observed: Counts {
                    added: 0,
                    removed: 0,
                },
                validated: counts.next(),
            });
        } else if matches!(raw, "*** Begin Patch" | "*** End Patch") {
            if let Some(file) = current.take() {
                truncated |= file.finish(&mut output);
            }
            output.push(raw.into());
        } else if let Some(file) = current.as_mut() {
            if raw.starts_with("*** Move to: ") {
                file.move_line = Some(raw);
            } else if raw == "*** End of File" {
                file.end_of_file = true;
            } else {
                file.observed.added += usize::from(raw.starts_with('+'));
                file.observed.removed += usize::from(raw.starts_with('-'));
                file.total_lines += 1;
                if file.lines.len() < PREVIEW_LINES_PER_FILE {
                    file.lines.push(raw);
                }
            }
        } else {
            if unstructured_lines < PREVIEW_LINES_PER_FILE {
                output.push(raw.into());
            }
            unstructured_lines += 1;
        }
    }
    if let Some(file) = current {
        truncated |= file.finish(&mut output);
    }
    if unstructured_lines > PREVIEW_LINES_PER_FILE {
        truncated = true;
        output.push(format!(
            "... {} more diff line(s)",
            unstructured_lines - PREVIEW_LINES_PER_FILE
        ));
    }
    (output.join("\n"), truncated)
}
