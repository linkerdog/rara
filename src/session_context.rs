use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::file_lock::AdvisoryFileLock;

const CONTEXT_SHARD_FILE: &str = "context.jsonl";
const CONTEXT_SHARD_CACHE_MAX_ENTRIES: usize = 256;

#[derive(Clone)]
struct ContextShardCacheEntry {
    modified: Option<SystemTime>,
    len: u64,
    checkpoints: Vec<SessionContextCheckpoint>,
}

fn context_shard_cache() -> &'static Mutex<HashMap<PathBuf, ContextShardCacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, ContextShardCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn prune_context_shard_cache(
    cache: &mut HashMap<PathBuf, ContextShardCacheEntry>,
    preserve: &Path,
) {
    while cache.len() >= CONTEXT_SHARD_CACHE_MAX_ENTRIES {
        let Some(key) = cache.keys().find(|key| key.as_path() != preserve).cloned() else {
            break;
        };
        cache.remove(&key);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionContextCheckpoint {
    pub session_id: String,
    pub turn_index: u32,
    pub text: String,
    #[serde(default)]
    pub vector: Vec<f32>,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionContextSearchHit {
    pub checkpoint: SessionContextCheckpoint,
    pub score: f32,
    pub vector_score: Option<f32>,
    pub keyword_score: f32,
}

pub fn context_shard_path(root_dir: &Path, session_id: &str) -> PathBuf {
    root_dir.join(session_id).join(CONTEXT_SHARD_FILE)
}

pub fn append_context_checkpoint(
    root_dir: &Path,
    session_id: &str,
    turn_index: u32,
    text: String,
    vector: Vec<f32>,
) -> Result<()> {
    let path = context_shard_path(root_dir, session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let checkpoint = SessionContextCheckpoint {
        session_id: session_id.to_string(),
        turn_index,
        text,
        vector,
        recorded_at: epoch_seconds(),
    };
    let _lock = AdvisoryFileLock::acquire(path.with_extension("lock"))?;
    let mut line = serde_json::to_vec(&checkpoint)?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open session context shard {}", path.display()))?;
    file.write_all(&line)?;
    file.sync_data()?;
    context_shard_cache()
        .lock()
        .expect("session context shard cache mutex poisoned")
        .remove(&path);
    if let Some(parent) = path.parent() {
        sync_parent_dir_best_effort(parent);
    }
    Ok(())
}

pub fn search_context_shards(
    root_dir: &Path,
    query: &str,
    query_vector: &[f32],
    limit: usize,
) -> Result<Vec<SessionContextSearchHit>> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let query_terms = tokenize(query);
    let mut latest_by_key = HashMap::<(String, u32), SessionContextCheckpoint>::new();
    for checkpoint in load_all_context_checkpoints(root_dir)? {
        latest_by_key.insert(
            (checkpoint.session_id.clone(), checkpoint.turn_index),
            checkpoint,
        );
    }
    let mut hits = latest_by_key
        .into_values()
        .filter_map(|checkpoint| {
            let keyword_score = keyword_match_score(&query_terms, &checkpoint.text);
            let vector_score = cosine_similarity(query_vector, &checkpoint.vector);
            let score = keyword_score * 2.0 + vector_score.unwrap_or(0.0);
            (score > 0.0).then_some(SessionContextSearchHit {
                checkpoint,
                score,
                vector_score,
                keyword_score,
            })
        })
        .collect::<Vec<_>>();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.checkpoint.recorded_at.cmp(&a.checkpoint.recorded_at))
            .then_with(|| a.checkpoint.session_id.cmp(&b.checkpoint.session_id))
            .then_with(|| a.checkpoint.turn_index.cmp(&b.checkpoint.turn_index))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn load_all_context_checkpoints(root_dir: &Path) -> Result<Vec<SessionContextCheckpoint>> {
    let mut checkpoints = Vec::new();
    if !root_dir.exists() {
        return Ok(checkpoints);
    }
    for entry in fs::read_dir(root_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join(CONTEXT_SHARD_FILE);
        if !path.exists() {
            continue;
        }
        checkpoints.extend(load_context_checkpoint_file_cached(&path)?);
    }
    Ok(checkpoints)
}

fn load_context_checkpoint_file_cached(path: &Path) -> Result<Vec<SessionContextCheckpoint>> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified().ok();
    let len = metadata.len();
    {
        let cache = context_shard_cache()
            .lock()
            .expect("session context shard cache mutex poisoned");
        if let Some(entry) = cache.get(path)
            && entry.modified == modified
            && entry.len == len
        {
            return Ok(entry.checkpoints.clone());
        }
    }

    let checkpoints = load_context_checkpoint_file(path)?;
    let mut cache = context_shard_cache()
        .lock()
        .expect("session context shard cache mutex poisoned");
    prune_context_shard_cache(&mut cache, path);
    cache.insert(
        path.to_path_buf(),
        ContextShardCacheEntry {
            modified,
            len,
            checkpoints: checkpoints.clone(),
        },
    );
    Ok(checkpoints)
}

fn load_context_checkpoint_file(path: &Path) -> Result<Vec<SessionContextCheckpoint>> {
    let file = fs::File::open(path)
        .with_context(|| format!("open session context shard {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut checkpoints = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(checkpoint) = serde_json::from_str::<SessionContextCheckpoint>(&line) {
            checkpoints.push(checkpoint);
        }
    }
    Ok(checkpoints)
}

fn keyword_match_score(query_terms: &[String], text: &str) -> f32 {
    if query_terms.is_empty() {
        return 0.0;
    }
    let text_terms = tokenize(text);
    let matched = query_terms
        .iter()
        .filter(|term| text_terms.iter().any(|text_term| text_term == *term))
        .count();
    matched as f32 / query_terms.len() as f32
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn cosine_similarity(query_vector: &[f32], vector: &[f32]) -> Option<f32> {
    if query_vector.is_empty() || query_vector.len() != vector.len() {
        return None;
    }
    let mut dot = 0.0f32;
    let mut query_norm = 0.0f32;
    let mut vector_norm = 0.0f32;
    for (left, right) in query_vector.iter().zip(vector.iter()) {
        dot += left * right;
        query_norm += left * left;
        vector_norm += right * right;
    }
    if query_norm == 0.0 || vector_norm == 0.0 {
        return None;
    }
    Some(dot / (query_norm.sqrt() * vector_norm.sqrt()))
}

fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(unix)]
fn sync_parent_dir_best_effort(parent: &Path) {
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir_best_effort(_parent: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_context_checkpoint_to_per_session_shard() -> Result<()> {
        let temp = tempfile::tempdir()?;

        append_context_checkpoint(
            temp.path(),
            "session-a",
            3,
            "approval denial should be visible to later turns".to_string(),
            vec![1.0, 0.0],
        )?;

        let path = context_shard_path(temp.path(), "session-a");
        assert!(path.exists());
        let checkpoints = load_context_checkpoint_file(&path)?;
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].session_id, "session-a");
        assert_eq!(checkpoints[0].turn_index, 3);
        assert!(checkpoints[0].text.contains("approval denial"));
        Ok(())
    }

    #[test]
    fn searches_context_shards_without_global_table() -> Result<()> {
        let temp = tempfile::tempdir()?;
        append_context_checkpoint(
            temp.path(),
            "session-a",
            1,
            "the runtime records denied approvals as errored tool results".to_string(),
            vec![1.0, 0.0],
        )?;
        append_context_checkpoint(
            temp.path(),
            "session-b",
            2,
            "model picker setup moved behind runtime bootstrap".to_string(),
            vec![0.0, 1.0],
        )?;

        let hits = search_context_shards(temp.path(), "denied approval", &[1.0, 0.0], 4)?;

        assert_eq!(hits[0].checkpoint.session_id, "session-a");
        assert!(hits[0].keyword_score > 0.0);
        assert!(hits[0].vector_score.is_some());
        Ok(())
    }

    #[test]
    fn search_keeps_latest_duplicate_checkpoint() -> Result<()> {
        let temp = tempfile::tempdir()?;
        append_context_checkpoint(
            temp.path(),
            "session-a",
            1,
            "old checkpoint text".to_string(),
            vec![0.0, 1.0],
        )?;
        append_context_checkpoint(
            temp.path(),
            "session-a",
            1,
            "new checkpoint text".to_string(),
            vec![1.0, 0.0],
        )?;

        let hits = search_context_shards(temp.path(), "new checkpoint", &[1.0, 0.0], 4)?;

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].checkpoint.text, "new checkpoint text");
        Ok(())
    }

    #[test]
    fn search_context_shards_invalidates_cache_after_append() -> Result<()> {
        let temp = tempfile::tempdir()?;
        append_context_checkpoint(
            temp.path(),
            "session-a",
            1,
            "old checkpoint text".to_string(),
            vec![1.0, 0.0],
        )?;
        let old_hits = search_context_shards(temp.path(), "old checkpoint", &[1.0, 0.0], 4)?;
        assert_eq!(old_hits.len(), 1);

        append_context_checkpoint(
            temp.path(),
            "session-a",
            2,
            "new checkpoint text".to_string(),
            vec![1.0, 0.0],
        )?;
        let new_hits = search_context_shards(temp.path(), "new checkpoint", &[1.0, 0.0], 4)?;

        assert!(!new_hits.is_empty());
        assert_eq!(new_hits[0].checkpoint.turn_index, 2);
        Ok(())
    }

    #[test]
    fn load_context_checkpoint_file_skips_malformed_lines() -> Result<()> {
        let temp = tempfile::tempdir()?;
        append_context_checkpoint(
            temp.path(),
            "session-a",
            1,
            "valid checkpoint text".to_string(),
            vec![1.0, 0.0],
        )?;
        let path = context_shard_path(temp.path(), "session-a");
        let mut file = OpenOptions::new().append(true).open(&path)?;
        file.write_all(b"{not valid json\n")?;
        file.write_all(
            serde_json::to_string(&SessionContextCheckpoint {
                session_id: "session-a".to_string(),
                turn_index: 2,
                text: "second valid checkpoint".to_string(),
                vector: vec![0.0, 1.0],
                recorded_at: 1,
            })?
            .as_bytes(),
        )?;
        file.write_all(b"\n")?;

        let checkpoints = load_context_checkpoint_file(&path)?;

        assert_eq!(checkpoints.len(), 2);
        assert_eq!(checkpoints[0].turn_index, 1);
        assert_eq!(checkpoints[1].turn_index, 2);
        Ok(())
    }

    #[test]
    fn context_shard_cache_is_bounded() -> Result<()> {
        context_shard_cache()
            .lock()
            .expect("session context shard cache mutex poisoned")
            .clear();
        let temp = tempfile::tempdir()?;

        for index in 0..=CONTEXT_SHARD_CACHE_MAX_ENTRIES {
            let session_id = format!("session-{index}");
            append_context_checkpoint(
                temp.path(),
                &session_id,
                index as u32,
                format!("checkpoint {index}"),
                vec![index as f32],
            )?;
            let path = context_shard_path(temp.path(), &session_id);
            let checkpoints = load_context_checkpoint_file_cached(&path)?;
            assert_eq!(checkpoints.len(), 1);
        }

        let cache = context_shard_cache()
            .lock()
            .expect("session context shard cache mutex poisoned");
        assert!(cache.len() <= CONTEXT_SHARD_CACHE_MAX_ENTRIES);
        Ok(())
    }
}
