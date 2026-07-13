use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use fs2::FileExt;
use hf_hub::progress::{DownloadEvent, ProgressEvent, ProgressHandler};
use hf_hub::{HFClient, HFClientSync, HFRepositorySync, RepoTypeModel, split_id};
use rara_persistence::redaction::{redact_secrets, sanitize_url_for_display};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::llm::{EmbeddingBackend, EmbeddingInputKind};
use crate::local_backend::{LocalProgressReporter, default_local_model_cache_dir};

include!("types.rs");
include!("backend.rs");
include!("status.rs");
include!("server.rs");
include!("prepare.rs");
include!("model.rs");
include!("progress.rs");
include!("util.rs");

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in result.as_slice() {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests;
