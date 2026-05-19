use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rara_memory::vectordb::{MemoryMetadata, VectorDB};
use rara_observability::{MemoryObservability, MemoryOperation, global_memory_observability};
use rara_persistence::atomic_file;
use rara_persistence::file_lock::AdvisoryFileLock;

use crate::llm::{EmbeddingBackend, EmbeddingInputKind, LlmBackend, LlmEmbeddingBackend};

const EXPERIENCES_TABLE: &str = "experiences";
const DEFAULT_IMPORTANCE: f32 = 0.5;
const HIGH_IMPORTANCE_RETENTION_THRESHOLD: f32 = 0.8;
const MEMORY_RECORD_INDEX_PLACEHOLDER: u32 = 0;
const MEMORY_RECORDS_FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLabel {
    Insight,
    Decision,
    Fact,
    Procedure,
    Experience,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    AgentTurn,
    UserCreated,
    ThreadDistill,
    SessionDistill,
    FileImport,
    ProtocolWrite,
    AutoMemory,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    User,
    Workspace,
    Project,
    Thread,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemorySourceSpan {
    pub start_turn_index: u32,
    pub end_turn_index: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub title: String,
    pub content: String,
    pub labels: Vec<MemoryLabel>,
    pub importance: f32,
    #[serde(default)]
    pub pinned: bool,
    pub source: MemorySource,
    pub scope: MemoryScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_span: Option<MemorySourceSpan>,
    pub created_at_unix_seconds: u64,
    pub updated_at_unix_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct NewMemoryRecord {
    pub title: Option<String>,
    pub content: String,
    pub labels: Vec<MemoryLabel>,
    pub importance: f32,
    pub pinned: bool,
    pub source: MemorySource,
    pub scope: MemoryScope,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub source_span: Option<MemorySourceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryPromotionTarget {
    Workspace {
        session_id: Option<String>,
        thread_id: Option<String>,
    },
    Thread {
        session_id: Option<String>,
        thread_id: String,
        source_span: Option<MemorySourceSpan>,
    },
    Session {
        session_id: String,
        source_span: Option<MemorySourceSpan>,
    },
}

impl NewMemoryRecord {
    pub fn experience(content: impl Into<String>) -> Self {
        Self {
            title: None,
            content: content.into(),
            labels: vec![MemoryLabel::Experience],
            importance: DEFAULT_IMPORTANCE,
            pinned: false,
            source: MemorySource::AgentTurn,
            scope: MemoryScope::Project,
            session_id: None,
            thread_id: None,
            source_span: None,
        }
    }

    pub fn promotion_base(source: MemorySource, target: MemoryPromotionTarget) -> Result<Self> {
        let (scope, session_id, thread_id, source_span) = match target {
            MemoryPromotionTarget::Workspace {
                session_id,
                thread_id,
            } => (
                MemoryScope::Workspace,
                normalized_optional_id(session_id),
                normalized_optional_id(thread_id),
                None,
            ),
            MemoryPromotionTarget::Thread {
                session_id,
                thread_id,
                source_span,
            } => (
                MemoryScope::Thread,
                normalized_optional_id(session_id),
                Some(required_memory_id(thread_id, "thread_id")?),
                source_span,
            ),
            MemoryPromotionTarget::Session {
                session_id,
                source_span,
            } => {
                let session_id = required_memory_id(session_id, "session_id")?;
                (
                    MemoryScope::Session,
                    Some(session_id.clone()),
                    Some(session_id),
                    source_span,
                )
            }
        };

        Ok(Self {
            title: None,
            content: String::new(),
            labels: vec![MemoryLabel::Experience],
            importance: 0.6,
            pinned: false,
            source,
            scope,
            session_id,
            thread_id,
            source_span,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemoryRecordPatch {
    pub title: Option<String>,
    pub content: Option<String>,
    pub labels: Option<Vec<MemoryLabel>>,
    pub importance: Option<f32>,
    pub pinned: Option<bool>,
    pub scope: Option<MemoryScope>,
    pub session_id: Option<Option<String>>,
    pub thread_id: Option<Option<String>>,
    pub source_span: Option<Option<MemorySourceSpan>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLabelCount {
    pub label: MemoryLabel,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRecordSearchHit {
    pub record: MemoryRecord,
    pub score: f32,
    pub vector_distance: Option<f32>,
    pub fts_score: Option<f32>,
}

pub struct MemoryStore {
    llm_backend: Arc<dyn LlmBackend>,
    embedding_backend: Arc<dyn EmbeddingBackend>,
    vdb: Arc<VectorDB>,
    records: MemoryRecordFileStore,
    observability: Arc<MemoryObservability>,
}
