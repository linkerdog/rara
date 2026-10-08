use std::fs;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use rara_memory::memory_handle::MemoryHandle;
use rara_persistence::thread_data::{
    PersistedCompactState, PersistedInteraction, PersistedPlanLifecycle, PersistedPlanStep,
    PersistedPromptRuntimeState, PersistedRuntimeRolloutItem, PersistedStructuredRolloutEvent,
    PersistedThreadLineage, PersistedThreadRecord, PersistedTurnEntry,
};
use rara_persistence::thread_metadata;
use rara_persistence::{thread_rollout_log, thread_turn_log};
use rara_state::state_db::StateDb;
use serde_json::Value;
use tempfile::tempdir;

use super::{
    RolloutItem, ThreadHistorySource, ThreadMetadataSource, ThreadNonTurnRolloutSource,
    ThreadRecorder, ThreadRuntimeLineage, ThreadRuntimeState, ThreadStore,
};
use crate::agent::Message;
use crate::llm::{ContentBlock, LlmBackend, LlmResponse, MockLlm, TokenUsage};
use crate::memory_store::{MemoryLabel, MemoryScope, MemorySource, MemoryStore};
use crate::session::{PersistedCompactionEvent, SessionManager};

mod materialization;
mod naming;
mod recording;
mod surfaces;
