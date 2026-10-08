use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use anyhow::Result;
use nucleo::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

use super::{FileMatch, FileSearchOptions, FileSearchResults, MatchType, build_walk};

/// Cooperative cancellation shared by a search owner and its worker.
#[derive(Clone, Default)]
pub struct SearchCancellation(Arc<AtomicBool>);

impl SearchCancellation {
    pub fn cancel(&self) {
        self.0.store(true, AtomicOrdering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(AtomicOrdering::Acquire)
    }
}

#[derive(Clone, Copy)]
pub struct IndexLimits {
    pub max_files: NonZeroUsize,
    pub max_path_bytes: NonZeroUsize,
}

/// A bounded workspace snapshot. Rebuild explicitly to observe filesystem changes.
pub struct FileSearchIndex {
    root: PathBuf,
    paths: Vec<String>,
    truncated: bool,
    skipped_non_utf8: usize,
}

#[derive(Debug)]
pub struct IndexedSearchResults {
    pub results: FileSearchResults,
    pub index_truncated: bool,
    pub skipped_non_utf8: usize,
}

impl FileSearchIndex {
    /// Returns `None` on cancellation. Filesystem/ignore errors remain errors.
    /// Only valid UTF-8 paths can be inserted into a text composer losslessly.
    pub fn build(
        root: PathBuf,
        options: &FileSearchOptions,
        limits: IndexLimits,
        cancellation: &SearchCancellation,
    ) -> Result<Option<Self>> {
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        let walk = build_walk(std::slice::from_ref(&root), options)?;
        let mut paths = Vec::new();
        let mut retained_bytes = 0usize;
        let mut skipped_non_utf8 = 0;
        let mut truncated = false;
        for entry in walk.build() {
            if cancellation.is_cancelled() {
                return Ok(None);
            }
            let entry = entry?;
            let is_file = match entry.file_type() {
                Some(kind) if kind.is_file() => true,
                Some(kind) if kind.is_symlink() => entry.path().metadata()?.is_file(),
                Some(_) | None => false,
            };
            if !is_file {
                continue;
            }
            let relative = entry.path().strip_prefix(&root)?;
            let Some(path) = relative.to_str() else {
                skipped_non_utf8 += 1;
                continue;
            };
            if paths.len() == limits.max_files.get()
                || retained_bytes.saturating_add(path.len()) > limits.max_path_bytes.get()
            {
                truncated = true;
                break;
            }
            retained_bytes += path.len();
            paths.push(path.to_owned());
        }
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        Ok(Some(Self {
            root,
            paths,
            truncated,
            skipped_non_utf8,
        }))
    }

    /// Reuses discovery and retains only the best `limit` matches while scoring.
    /// Cancellation never returns a partial result as if it were complete.
    pub fn search(
        &self,
        query: &str,
        limit: NonZeroUsize,
        cancellation: &SearchCancellation,
    ) -> Option<IndexedSearchResults> {
        self.search_until(query, limit, || cancellation.is_cancelled())
    }

    fn search_until(
        &self,
        query: &str,
        limit: NonZeroUsize,
        mut cancelled: impl FnMut() -> bool,
    ) -> Option<IndexedSearchResults> {
        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut buffer = Vec::new();
        let mut matches = BinaryHeap::new();
        let mut total_match_count = 0;
        for path in &self.paths {
            if cancelled() {
                return None;
            }
            buffer.clear();
            let Some(score) = pattern.score(Utf32Str::new(path, &mut buffer), &mut matcher) else {
                continue;
            };
            total_match_count += 1;
            let candidate = RankedMatch { score, path };
            if matches.len() < limit.get() {
                matches.push(candidate);
            } else if let Some(mut worst) = matches.peek_mut()
                && candidate < *worst
            {
                *worst = candidate;
            }
        }
        if cancelled() {
            return None;
        }
        let matches = matches
            .into_sorted_vec()
            .into_iter()
            .map(|candidate| FileMatch {
                score: candidate.score,
                path: PathBuf::from(candidate.path),
                root: self.root.clone(),
                match_type: MatchType::File,
            })
            .collect();
        Some(IndexedSearchResults {
            results: FileSearchResults {
                matches,
                total_match_count,
                scanned_entry_count: self.paths.len(),
                truncated: total_match_count > limit.get(),
            },
            index_truncated: self.truncated,
            skipped_non_utf8: self.skipped_non_utf8,
        })
    }
}

#[derive(Eq, PartialEq)]
struct RankedMatch<'a> {
    score: u32,
    path: &'a str,
}

impl Ord for RankedMatch<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        // The worst retained match is the heap root; final ascending order
        // matches the existing API's descending score / ascending path order.
        other
            .score
            .cmp(&self.score)
            .then_with(|| self.path.cmp(other.path))
    }
}

impl PartialOrd for RankedMatch<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests;
