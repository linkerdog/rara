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
use hf_hub::api::Progress as HfProgress;
use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Cache, Repo, RepoType};
use nix::unistd::Pid;
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

pub(crate) fn sha256_hex(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests;
