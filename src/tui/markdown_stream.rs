use std::path::{Path, PathBuf};

use ratatui::text::Line;

use crate::tui::{line_utils::is_blank_line_spaces_only, markdown};

pub(crate) struct MarkdownStreamCollector {
    buffer: String,
    committed_line_count: usize,
    width: Option<usize>,
    cwd: PathBuf,
}

impl MarkdownStreamCollector {
    pub fn new(width: Option<usize>, cwd: &Path) -> Self {
        Self {
            buffer: String::new(),
            committed_line_count: 0,
            width,
            cwd: cwd.to_path_buf(),
        }
    }

    pub fn push_delta(&mut self, delta: &str) {
        self.buffer.push_str(delta);
    }

    pub fn commit_complete_lines(&mut self) -> Vec<Line<'static>> {
        let source = self.buffer.clone();
        let Some(last_newline_idx) = source.rfind('\n') else {
            return Vec::new();
        };
        let source = source[..=last_newline_idx].to_string();
        let mut rendered = Vec::new();
        markdown::append_markdown(&source, self.width, Some(self.cwd.as_path()), &mut rendered);
        let mut complete_line_count = rendered.len();
        if complete_line_count > 0 && is_blank_line_spaces_only(&rendered[complete_line_count - 1])
        {
            complete_line_count -= 1;
        }

        if self.committed_line_count >= complete_line_count {
            return Vec::new();
        }

        let out = rendered[self.committed_line_count..complete_line_count].to_vec();
        self.committed_line_count = complete_line_count;
        out
    }

    pub fn preview_lines(&self) -> Vec<Line<'static>> {
        if self.buffer.is_empty() {
            return Vec::new();
        }

        let mut rendered = Vec::new();
        markdown::append_markdown(
            &self.buffer,
            self.width,
            Some(self.cwd.as_path()),
            &mut rendered,
        );
        if self.committed_line_count >= rendered.len() {
            Vec::new()
        } else {
            rendered[self.committed_line_count..].to_vec()
        }
    }

    pub fn finalize_and_drain(&mut self) -> Vec<Line<'static>> {
        if self.buffer.is_empty() {
            return Vec::new();
        }

        let mut source = self.buffer.clone();
        if !source.ends_with('\n') {
            source.push('\n');
        }

        let mut rendered = Vec::new();
        markdown::append_markdown(&source, self.width, Some(self.cwd.as_path()), &mut rendered);
        let out = if self.committed_line_count >= rendered.len() {
            Vec::new()
        } else {
            rendered[self.committed_line_count..].to_vec()
        };

        self.buffer.clear();
        self.committed_line_count = 0;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cwd() -> PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn no_commit_until_newline() {
        let mut collector = MarkdownStreamCollector::new(None, &test_cwd());
        collector.push_delta("Hello");
        assert!(collector.commit_complete_lines().is_empty());
        collector.push_delta("\n");
        assert_eq!(collector.commit_complete_lines().len(), 1);
    }

    #[test]
    fn preview_only_returns_uncommitted_tail() {
        let mut collector = MarkdownStreamCollector::new(None, &test_cwd());
        collector.push_delta("Hello\nWorld");
        let committed = collector.commit_complete_lines();
        assert_eq!(committed.len(), 1);
        let preview = collector.preview_lines();
        assert_eq!(preview.len(), 1);
    }

    #[test]
    fn finalize_commits_partial_line() {
        let mut collector = MarkdownStreamCollector::new(None, &test_cwd());
        collector.push_delta("Line without newline");
        let out = collector.finalize_and_drain();
        assert_eq!(out.len(), 1);
        assert!(collector.preview_lines().is_empty());
    }
}
