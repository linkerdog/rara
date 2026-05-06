use std::cmp::Ordering;
use std::num::NonZero;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use nucleo::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchType {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileMatch {
    pub score: u32,
    pub path: PathBuf,
    pub root: PathBuf,
    pub match_type: MatchType,
}

impl FileMatch {
    pub fn full_path(&self) -> PathBuf {
        self.root.join(&self.path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileEntry {
    pub path: PathBuf,
    pub root: PathBuf,
    pub match_type: MatchType,
}

impl FileEntry {
    pub fn full_path(&self) -> PathBuf {
        self.root.join(&self.path)
    }
}

#[derive(Debug, Clone)]
pub struct FileSearchOptions {
    pub limit: NonZero<usize>,
    pub exclude: Vec<String>,
    pub respect_gitignore: bool,
    pub include_hidden: bool,
    pub follow_links: bool,
}

impl Default for FileSearchOptions {
    fn default() -> Self {
        Self {
            #[expect(clippy::unwrap_used)]
            limit: NonZero::new(64).unwrap(),
            exclude: Vec::new(),
            respect_gitignore: true,
            include_hidden: true,
            follow_links: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileSearchResults {
    pub matches: Vec<FileMatch>,
    pub total_match_count: usize,
    pub scanned_entry_count: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileListResults {
    pub entries: Vec<FileEntry>,
    pub total_entry_count: usize,
    pub truncated: bool,
}

pub fn search_files(
    pattern_text: &str,
    roots: Vec<PathBuf>,
    options: FileSearchOptions,
) -> Result<FileSearchResults> {
    let entries = collect_entries(&roots, &options)?;
    let pattern = Pattern::new(
        pattern_text,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut utf32buf = Vec::<char>::new();
    let mut matches = entries
        .iter()
        .filter_map(|entry| {
            let text = entry.path.to_string_lossy();
            utf32buf.clear();
            let haystack = Utf32Str::new(&text, &mut utf32buf);
            pattern
                .score(haystack, &mut matcher)
                .map(|score| FileMatch {
                    score,
                    path: entry.path.clone(),
                    root: entry.root.clone(),
                    match_type: entry.match_type,
                })
        })
        .collect::<Vec<_>>();
    matches.sort_by(cmp_match);

    let total_match_count = matches.len();
    let limit = options.limit.get();
    let truncated = total_match_count > limit;
    matches.truncate(limit);

    Ok(FileSearchResults {
        matches,
        total_match_count,
        scanned_entry_count: entries.len(),
        truncated,
    })
}

pub fn list_files(root: impl Into<PathBuf>, options: FileSearchOptions) -> Result<FileListResults> {
    let mut entries = collect_entries(&[root.into()], &options)?;
    entries.retain(|entry| entry.match_type == MatchType::File);
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let total_entry_count = entries.len();
    let limit = options.limit.get();
    let truncated = total_entry_count > limit;
    entries.truncate(limit);

    Ok(FileListResults {
        entries,
        total_entry_count,
        truncated,
    })
}

fn collect_entries(roots: &[PathBuf], options: &FileSearchOptions) -> Result<Vec<FileEntry>> {
    let Some(first_root) = roots.first() else {
        anyhow::bail!("at least one search root is required");
    };
    let override_matcher = build_override_matcher(first_root, &options.exclude)?;
    let mut walk_builder = WalkBuilder::new(first_root);
    for root in roots.iter().skip(1) {
        walk_builder.add(root);
    }
    walk_builder
        .hidden(!options.include_hidden)
        .follow_links(options.follow_links)
        .require_git(true);
    if !options.respect_gitignore {
        walk_builder
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .ignore(false)
            .parents(false);
    }
    if let Some(override_matcher) = override_matcher {
        walk_builder.overrides(override_matcher);
    }

    let mut entries = Vec::new();
    for entry in walk_builder.build() {
        let entry = entry?;
        let path = entry.path();
        if path == first_root {
            continue;
        }
        let Some((root, relative_path)) = relative_entry(path, roots) else {
            continue;
        };
        let match_type = if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir())
        {
            MatchType::Directory
        } else {
            MatchType::File
        };
        entries.push(FileEntry {
            path: relative_path.to_path_buf(),
            root: root.to_path_buf(),
            match_type,
        });
    }
    Ok(entries)
}

fn build_override_matcher(
    root: &Path,
    exclude: &[String],
) -> Result<Option<ignore::overrides::Override>> {
    if exclude.is_empty() {
        return Ok(None);
    }
    let mut builder = OverrideBuilder::new(root);
    for pattern in exclude {
        builder
            .add(&format!("!{pattern}"))
            .with_context(|| format!("invalid exclude pattern {pattern:?}"))?;
    }
    Ok(Some(builder.build()?))
}

fn relative_entry<'a>(path: &'a Path, roots: &'a [PathBuf]) -> Option<(&'a Path, &'a Path)> {
    let mut best_match: Option<&PathBuf> = None;
    for root in roots {
        if path.strip_prefix(root).is_ok() {
            match best_match {
                Some(best) if best.components().count() >= root.components().count() => {}
                _ => best_match = Some(root),
            }
        }
    }
    let root = best_match?;
    let relative = path.strip_prefix(root).ok()?;
    Some((root.as_path(), relative))
}

fn cmp_match(a: &FileMatch, b: &FileMatch) -> Ordering {
    b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn list_files_respects_gitignore_inside_git_repo() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let root = tempdir.path();
        fs::create_dir(root.join(".git")).expect("git dir");
        fs::create_dir(root.join("src")).expect("src dir");
        fs::create_dir(root.join("target")).expect("target dir");
        fs::write(root.join(".gitignore"), "target/\n").expect("gitignore");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("source");
        fs::write(root.join("target/app"), "bin").expect("ignored");

        let result = list_files(root, FileSearchOptions::default()).expect("list files");
        let rendered = render_entries(&result.entries);

        assert!(contains_entry(&rendered, "src/main.rs"));
        assert!(!contains_entry(&rendered, "target/app"));
    }

    #[test]
    fn list_files_can_include_ignored_entries() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let root = tempdir.path();
        fs::create_dir(root.join(".git")).expect("git dir");
        fs::create_dir(root.join("target")).expect("target dir");
        fs::write(root.join(".gitignore"), "target/\n").expect("gitignore");
        fs::write(root.join("target/app"), "bin").expect("ignored");

        let result = list_files(
            root,
            FileSearchOptions {
                respect_gitignore: false,
                ..FileSearchOptions::default()
            },
        )
        .expect("list files");

        assert!(contains_entry(
            &render_entries(&result.entries),
            "target/app"
        ));
    }

    #[test]
    fn list_files_is_bounded_and_stably_sorted() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let root = tempdir.path();
        fs::write(root.join("b.txt"), "b").expect("b");
        fs::write(root.join("a.txt"), "a").expect("a");
        fs::write(root.join("c.txt"), "c").expect("c");

        let result = list_files(
            root,
            FileSearchOptions {
                #[expect(clippy::unwrap_used)]
                limit: NonZero::new(2).unwrap(),
                ..FileSearchOptions::default()
            },
        )
        .expect("list files");

        assert_eq!(render_entries(&result.entries), vec!["a.txt", "b.txt"]);
        assert_eq!(result.total_entry_count, 3);
        assert!(result.truncated);
    }

    #[test]
    fn search_files_ranks_fuzzy_matches() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let root = tempdir.path();
        fs::create_dir(root.join("src")).expect("src");
        fs::write(root.join("src/context_display.rs"), "").expect("context");
        fs::write(root.join("src/terminal.rs"), "").expect("terminal");

        let result = search_files(
            "ctxdisplay",
            vec![root.to_path_buf()],
            FileSearchOptions::default(),
        )
        .expect("search files");

        assert_eq!(
            result.matches[0].path,
            PathBuf::from("src/context_display.rs")
        );
    }

    fn render_entries(entries: &[FileEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().into_owned())
            .collect()
    }

    fn contains_entry(entries: &[String], expected: &str) -> bool {
        entries.iter().any(|entry| entry == expected)
    }
}
