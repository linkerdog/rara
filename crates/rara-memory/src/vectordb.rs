use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, UInt32Type};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
    UInt32Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures::TryStreamExt;
use lancedb::index::Index;
use lancedb::index::scalar::FtsIndexBuilder;
use lancedb::index::scalar::FullTextSearchQuery;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{Connection, Error as LanceDbError, Table, connect};
use rara_persistence::file_lock::AdvisoryFileLock;
use tokio::sync::{Mutex, OnceCell};

const MEMORY_ID_COLUMN: &str = "id";
const SESSION_ID_COLUMN: &str = "session_id";
const TURN_INDEX_COLUMN: &str = "turn_index";
const TEXT_COLUMN: &str = "text";
const VECTOR_COLUMN: &str = "vector";
const VECTOR_DISTANCE_COLUMN: &str = "_distance";
const FTS_SCORE_COLUMN: &str = "_score";

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
pub struct MemoryMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub session_id: String,
    pub turn_index: u32,
    pub text: String,
}

impl MemoryMetadata {
    fn stable_id(&self) -> String {
        self.id
            .clone()
            .unwrap_or_else(|| format!("{}:{}", self.session_id, self.turn_index))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemorySearchHit {
    pub metadata: MemoryMetadata,
    pub score: f32,
    pub vector_distance: Option<f32>,
    pub fts_score: Option<f32>,
}

pub struct VectorDB {
    uri: String,
    write_lock_path: PathBuf,
    db: OnceCell<Connection>,
    fts_indexed_tables: Mutex<HashMap<String, Arc<OnceCell<()>>>>,
}

impl VectorDB {
    pub fn new(uri: &str) -> Self {
        Self {
            uri: uri.to_string(),
            write_lock_path: PathBuf::from(format!("{uri}.lock")),
            db: OnceCell::new(),
            fts_indexed_tables: Mutex::new(HashMap::new()),
        }
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub async fn search_with_metadata(
        &self,
        table_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<(MemoryMetadata, f32)>> {
        if query_vector.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        Ok(self
            .vector_search_hits(table_name, query_vector, limit)
            .await?
            .into_iter()
            .map(|hit| (hit.metadata, hit.score))
            .collect())
    }

    pub async fn full_text_search_with_metadata(
        &self,
        table_name: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemorySearchHit>> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let Some(table) = self.open_table_if_exists(table_name).await? else {
            return Ok(Vec::new());
        };
        self.ensure_fts_index(&table).await?;
        let batches = table
            .query()
            .full_text_search(FullTextSearchQuery::new(query.to_string()))
            .limit(limit)
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        memory_hits_from_batches(&batches)
    }

    pub async fn hybrid_search_with_metadata(
        &self,
        table_name: &str,
        query: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<MemorySearchHit>> {
        if query.trim().is_empty() || query_vector.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let vector_limit = limit.saturating_mul(2).max(limit);
        let fts_limit = limit.saturating_mul(2).max(limit);
        let vector_hits = self
            .vector_search_hits(table_name, query_vector, vector_limit)
            .await?;
        let fts_hits = self
            .full_text_search_with_metadata(table_name, query, fts_limit)
            .await?;

        Ok(fuse_hybrid_hits(vector_hits, fts_hits, limit))
    }

    async fn vector_search_hits(
        &self,
        table_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<MemorySearchHit>> {
        if query_vector.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let Some(table) = self.open_table_if_exists(table_name).await? else {
            return Ok(Vec::new());
        };
        let batches = table
            .query()
            .nearest_to(query_vector)?
            .limit(limit)
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        memory_hits_from_batches(&batches)
    }

    pub async fn upsert_turn(
        &self,
        table_name: &str,
        metadata: MemoryMetadata,
        vector: Vec<f32>,
    ) -> Result<()> {
        if vector.is_empty() {
            return Ok(());
        }
        let _write_lock = self.acquire_write_lock().await?;
        let table = self.open_or_create_table(table_name, vector.len()).await?;
        let batch = memory_record_batch(&metadata, vector)?;
        let schema = batch.schema();
        let reader = RecordBatchIterator::new([Ok(batch)], schema);
        let mut merge = table.merge_insert(&[MEMORY_ID_COLUMN]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge
            .execute(Box::new(reader))
            .await
            .context("merge memory record into LanceDB table")?;
        self.ensure_fts_index_locked(&table).await?;
        Ok(())
    }

    async fn acquire_write_lock(&self) -> Result<AdvisoryFileLock> {
        let path = self.write_lock_path.clone();
        tokio::task::spawn_blocking(move || AdvisoryFileLock::acquire(path))
            .await
            .context("join LanceDB write lock task")?
    }

    async fn db(&self) -> Result<&Connection> {
        self.db
            .get_or_try_init(|| async {
                connect(&self.uri)
                    .execute()
                    .await
                    .with_context(|| format!("connect LanceDB at {}", self.uri))
            })
            .await
    }

    async fn open_or_create_table(&self, table_name: &str, vector_dim: usize) -> Result<Table> {
        let db = self.db().await?;
        if let Some(table) = self.open_table_if_exists(table_name).await? {
            if let Err(e) = validate_table_vector_dim(&table, vector_dim).await {
                log::warn!(
                    "LanceDB vector dimension mismatch for table {table_name}; \
                     dropping and recreating. {e}"
                );
                db.drop_table(table_name, &[])
                    .await
                    .with_context(|| format!("drop mismatched LanceDB table {table_name}"))?;
            } else {
                return Ok(table);
            }
        }
        db.create_empty_table(table_name, memory_schema(vector_dim))
            .execute()
            .await
            .with_context(|| format!("create LanceDB table {table_name}"))
    }

    async fn open_table_if_exists(&self, table_name: &str) -> Result<Option<Table>> {
        let db = self.db().await?;
        match db.open_table(table_name).execute().await {
            Ok(table) => Ok(Some(table)),
            Err(LanceDbError::TableNotFound { .. }) => Ok(None),
            Err(err) => Err(err).with_context(|| format!("open LanceDB table {table_name}")),
        }
    }

    async fn ensure_fts_index(&self, table: &Table) -> Result<()> {
        let _write_lock = self.acquire_write_lock().await?;
        self.ensure_fts_index_locked(table).await
    }

    async fn ensure_fts_index_locked(&self, table: &Table) -> Result<()> {
        let table_name = table.name().to_string();
        let fts_once = {
            let mut indexed_tables = self.fts_indexed_tables.lock().await;
            indexed_tables
                .entry(table_name)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        fts_once
            .get_or_try_init(|| async {
                let indices = table.list_indices().await?;
                let has_text_fts = indices.iter().any(|index| {
                    index.columns.iter().any(|column| column == TEXT_COLUMN)
                        && index.index_type.to_string() == "FTS"
                });
                if !has_text_fts {
                    table
                        .create_index(&[TEXT_COLUMN], Index::FTS(FtsIndexBuilder::default()))
                        .execute()
                        .await
                        .context("create LanceDB FTS index on memory text")?;
                }
                Ok(())
            })
            .await
            .map(|_| ())
    }
}

async fn validate_table_vector_dim(table: &Table, expected_dim: usize) -> Result<()> {
    let schema = table.schema().await?;
    let actual_dim = vector_dim_from_schema(&schema)?;
    if actual_dim != expected_dim {
        anyhow::bail!(
            "LanceDB memory vector dimension mismatch for table {}: expected {}, got {}",
            table.name(),
            expected_dim,
            actual_dim
        );
    }
    Ok(())
}

fn vector_dim_from_schema(schema: &Schema) -> Result<usize> {
    let vector_field = schema
        .field_with_name(VECTOR_COLUMN)
        .with_context(|| format!("missing {VECTOR_COLUMN} column in LanceDB memory schema"))?;
    let (_, dim) = vector_field
        .data_type()
        .as_fixed_size_list()
        .with_context(|| format!("LanceDB memory column {VECTOR_COLUMN} is not FixedSizeList"))?;
    usize::try_from(dim).context("LanceDB memory vector dimension is negative")
}

fn memory_schema(vector_dim: usize) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(MEMORY_ID_COLUMN, DataType::Utf8, false),
        Field::new(SESSION_ID_COLUMN, DataType::Utf8, false),
        Field::new(TURN_INDEX_COLUMN, DataType::UInt32, false),
        Field::new(TEXT_COLUMN, DataType::Utf8, false),
        Field::new(
            VECTOR_COLUMN,
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                vector_dim as i32,
            ),
            false,
        ),
    ]))
}

fn memory_record_batch(metadata: &MemoryMetadata, vector: Vec<f32>) -> Result<RecordBatch> {
    let schema = memory_schema(vector.len());
    let id = metadata.stable_id();
    let vector_dim = i32::try_from(vector_dim_from_schema(&schema)?)
        .context("LanceDB memory vector dimension exceeds i32")?;
    let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        [Some(vector.into_iter().map(Some).collect::<Vec<_>>())],
        vector_dim,
    );
    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![id])),
            Arc::new(StringArray::from(vec![metadata.session_id.clone()])),
            Arc::new(UInt32Array::from(vec![metadata.turn_index])),
            Arc::new(StringArray::from(vec![metadata.text.clone()])),
            Arc::new(vectors),
        ],
    )?)
}

fn memory_hits_from_batches(batches: &[RecordBatch]) -> Result<Vec<MemorySearchHit>> {
    let mut hits = Vec::new();
    for batch in batches {
        let ids = string_column(batch, MEMORY_ID_COLUMN)?;
        let session_ids = string_column(batch, SESSION_ID_COLUMN)?;
        let turn_indices = batch
            .column_by_name(TURN_INDEX_COLUMN)
            .context("missing turn_index column in LanceDB memory result")?
            .as_primitive::<UInt32Type>();
        let texts = string_column(batch, TEXT_COLUMN)?;
        let vector_distances = optional_f32_column(batch, VECTOR_DISTANCE_COLUMN);
        let fts_scores = optional_f32_column(batch, FTS_SCORE_COLUMN);
        for row in 0..batch.num_rows() {
            let vector_distance = optional_f32_value(vector_distances, row);
            let fts_score = optional_f32_value(fts_scores, row);
            hits.push(MemorySearchHit {
                metadata: MemoryMetadata {
                    id: Some(ids.value(row).to_string()),
                    session_id: session_ids.value(row).to_string(),
                    turn_index: turn_indices.value(row),
                    text: texts.value(row).to_string(),
                },
                score: combined_score(vector_distance, fts_score),
                vector_distance,
                fts_score,
            });
        }
    }
    Ok(hits)
}

fn string_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .with_context(|| format!("missing {name} column in LanceDB memory result"))?
        .as_any()
        .downcast_ref::<StringArray>()
        .with_context(|| format!("LanceDB memory column {name} is not Utf8"))
}

fn optional_f32_column<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a Float32Array> {
    let column = batch.column_by_name(name)?;
    column.as_any().downcast_ref::<Float32Array>()
}

fn optional_f32_value(values: Option<&Float32Array>, row: usize) -> Option<f32> {
    let values = values?;
    (!values.is_null(row)).then(|| values.value(row))
}

fn combined_score(vector_distance: Option<f32>, fts_score: Option<f32>) -> f32 {
    match (vector_distance, fts_score) {
        (Some(distance), Some(score)) => score + (1.0 / (1.0 + distance.max(0.0))),
        (Some(distance), None) => 1.0 / (1.0 + distance.max(0.0)),
        (None, Some(score)) => score,
        (None, None) => 0.0,
    }
}

#[derive(Default)]
struct HybridAccumulator {
    metadata: Option<MemoryMetadata>,
    vector_distance: Option<f32>,
    fts_score: Option<f32>,
    vector_rank: Option<usize>,
    fts_rank: Option<usize>,
}

fn fuse_hybrid_hits(
    vector_hits: Vec<MemorySearchHit>,
    fts_hits: Vec<MemorySearchHit>,
    limit: usize,
) -> Vec<MemorySearchHit> {
    let mut by_id = HashMap::<String, HybridAccumulator>::new();
    for (idx, hit) in vector_hits.into_iter().enumerate() {
        let id = hit.metadata.stable_id();
        let entry = by_id.entry(id).or_default();
        entry.metadata.get_or_insert(hit.metadata);
        entry.vector_distance = hit.vector_distance;
        entry.vector_rank = Some(idx + 1);
    }
    for (idx, hit) in fts_hits.into_iter().enumerate() {
        let id = hit.metadata.stable_id();
        let entry = by_id.entry(id).or_default();
        entry.metadata.get_or_insert(hit.metadata);
        entry.fts_score = hit.fts_score;
        entry.fts_rank = Some(idx + 1);
    }

    let mut hits = by_id
        .into_values()
        .filter_map(|entry| {
            let metadata = entry.metadata?;
            Some(MemorySearchHit {
                metadata,
                score: reciprocal_rank_score(entry.vector_rank, entry.fts_rank),
                vector_distance: entry.vector_distance,
                fts_score: entry.fts_score,
            })
        })
        .collect::<Vec<_>>();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(limit);
    hits
}

fn reciprocal_rank_score(vector_rank: Option<usize>, fts_rank: Option<usize>) -> f32 {
    const RRF_K: f32 = 60.0;
    let score_for_rank = |rank: usize| 1.0 / (RRF_K + rank as f32);
    vector_rank.map(score_for_rank).unwrap_or_default()
        + fts_rank.map(score_for_rank).unwrap_or_default()
}

trait FixedSizeListDataTypeExt {
    fn as_fixed_size_list(&self) -> Option<(&Field, i32)>;
}

impl FixedSizeListDataTypeExt for DataType {
    fn as_fixed_size_list(&self) -> Option<(&Field, i32)> {
        match self {
            DataType::FixedSizeList(field, dim) => Some((field.as_ref(), *dim)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lancedb_memory_index_supports_vector_fts_and_hybrid_search() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = VectorDB::new(temp.path().to_str().expect("utf8 path"));
        db.upsert_turn(
            "conversations",
            MemoryMetadata {
                id: None,
                session_id: "session-a".to_string(),
                turn_index: 1,
                text: "Fix DeepSeek DSML parsing with a structured parser".to_string(),
            },
            vec![1.0, 0.0, 0.0],
        )
        .await
        .expect("insert first memory");
        db.upsert_turn(
            "conversations",
            MemoryMetadata {
                id: None,
                session_id: "session-b".to_string(),
                turn_index: 2,
                text: "Improve viewport rendering for queued approvals".to_string(),
            },
            vec![0.0, 1.0, 0.0],
        )
        .await
        .expect("insert second memory");

        let fts_hits = db
            .full_text_search_with_metadata("conversations", "DeepSeek DSML", 5)
            .await
            .expect("fts search");
        assert_eq!(fts_hits[0].metadata.session_id, "session-a");
        assert!(fts_hits[0].fts_score.is_some());

        let vector_hits = db
            .search_with_metadata("conversations", vec![0.0, 1.0, 0.0], 1)
            .await
            .expect("vector search");
        assert_eq!(vector_hits[0].0.session_id, "session-b");

        let hybrid_hits = db
            .hybrid_search_with_metadata("conversations", "DeepSeek parser", vec![1.0, 0.0, 0.0], 5)
            .await
            .expect("hybrid search");
        assert!(
            hybrid_hits
                .iter()
                .any(|hit| hit.metadata.session_id == "session-a")
        );
    }

    #[tokio::test]
    async fn lancedb_memory_search_does_not_create_table_with_default_dimension() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = VectorDB::new(temp.path().to_str().expect("utf8 path"));

        let empty = db
            .full_text_search_with_metadata("experiences", "missing", 5)
            .await
            .expect("missing table search");
        assert!(empty.is_empty());

        db.upsert_turn(
            "experiences",
            MemoryMetadata {
                id: None,
                session_id: "session-a".to_string(),
                turn_index: 1,
                text: "Remember exact config key names".to_string(),
            },
            vec![1.0, 0.0, 0.0, 0.0],
        )
        .await
        .expect("insert after missing search should use real vector dimension");
        let hits = db
            .search_with_metadata("experiences", vec![1.0, 0.0, 0.0, 0.0], 1)
            .await
            .expect("vector search");
        assert_eq!(hits[0].0.text, "Remember exact config key names");
    }

    #[tokio::test]
    async fn lancedb_memory_upsert_uses_explicit_ids_without_turn_collision() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = VectorDB::new(temp.path().to_str().expect("utf8 path"));

        db.upsert_turn(
            "experiences",
            MemoryMetadata {
                id: Some("memory-a".to_string()),
                session_id: "project".to_string(),
                turn_index: 7,
                text: "First durable memory".to_string(),
            },
            vec![1.0, 0.0, 0.0],
        )
        .await
        .expect("insert first explicit memory");
        db.upsert_turn(
            "experiences",
            MemoryMetadata {
                id: Some("memory-b".to_string()),
                session_id: "project".to_string(),
                turn_index: 7,
                text: "Second durable memory".to_string(),
            },
            vec![0.9, 0.1, 0.0],
        )
        .await
        .expect("insert second explicit memory");

        let hits = db
            .search_with_metadata("experiences", vec![1.0, 0.0, 0.0], 5)
            .await
            .expect("vector search");
        let texts = hits
            .into_iter()
            .map(|(metadata, _)| metadata.text)
            .collect::<Vec<_>>();
        assert!(texts.contains(&"First durable memory".to_string()));
        assert!(texts.contains(&"Second durable memory".to_string()));
    }

    #[tokio::test]
    async fn lancedb_memory_upsert_reports_dimension_mismatch_without_deleting_existing_row() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = VectorDB::new(temp.path().to_str().expect("utf8 path"));

        db.upsert_turn(
            "experiences",
            MemoryMetadata {
                id: Some("memory-a".to_string()),
                session_id: "project".to_string(),
                turn_index: 1,
                text: "Original memory".to_string(),
            },
            vec![1.0, 0.0, 0.0],
        )
        .await
        .expect("insert original memory");

        let err = db
            .upsert_turn(
                "experiences",
                MemoryMetadata {
                    id: Some("memory-a".to_string()),
                    session_id: "project".to_string(),
                    turn_index: 1,
                    text: "Wrong dimension replacement".to_string(),
                },
                vec![1.0, 0.0, 0.0, 0.0],
            )
            .await
            .expect_err("dimension mismatch should fail");
        assert!(err.to_string().contains("vector dimension mismatch"));

        let hits = db
            .search_with_metadata("experiences", vec![1.0, 0.0, 0.0], 1)
            .await
            .expect("vector search");
        assert_eq!(hits[0].0.text, "Original memory");
    }
}
