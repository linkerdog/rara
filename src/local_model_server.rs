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
use rara_persistence::redaction::{redact_secrets, sanitize_url_for_display};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::llm::{EmbeddingBackend, EmbeddingInputKind};
use crate::local_backend::{LocalProgressReporter, default_local_model_cache_dir};

const MODEL_SERVER: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/components/model_server/rara_model_server.py"
));
const MODEL_SERVER_NAME: &str = "rara_model_server.py";

const REQUIREMENTS_MACOS_ARM64: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/components/model_server/requirements-macos-arm64.txt"
));
const REQUIREMENTS_MACOS_ARM64_NAME: &str = "requirements-macos-arm64.txt";

const REQUIREMENTS_PORTABLE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/components/model_server/requirements-portable.txt"
));
const REQUIREMENTS_PORTABLE_NAME: &str = "requirements-portable.txt";
const MLX_QWEN3_MODEL_ID: &str = "mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ";
const FASTEMBED_BGE_M3_MODEL_ID: &str = "BAAI/bge-m3";
const DEFAULT_MODEL_SERVER_HOST: &str = "127.0.0.1";
const DEFAULT_MODEL_SERVER_PORT: u16 = 18181;
const SERVER_METADATA_NAME: &str = "server.json";
const STARTUP_LOCK_NAME: &str = "startup.lock";
const REQUIREMENTS_MARKER_NAME: &str = "requirements-installed.json";
const MODEL_SNAPSHOT_MARKER_NAME: &str = "model-snapshot.json";
const HEALTH_TIMEOUT: Duration = Duration::from_millis(300);
const PREPARE_TIMEOUT: Duration = Duration::from_secs(30);
const STARTUP_HEALTH_ATTEMPTS: usize = 10;
const STARTUP_HEALTH_DELAY: Duration = Duration::from_millis(100);
const MODEL_REVISION: &str = "main";
static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotPreparation {
    RustManaged {
        required_files: SnapshotRequiredFiles,
    },
    PythonManaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotRequiredFiles {
    MlxQwen3,
}

#[derive(Debug, Clone, Copy)]
struct LocalEmbeddingModelProfile {
    backend: &'static str,
    model: &'static str,
    revision: &'static str,
    snapshot_preparation: SnapshotPreparation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalModelServerState {
    Ready,
    Starting,
    WaitingForServer,
    CreatingVenv,
    InstallingDependencies,
    PreparingModel,
    PreparedButStopped,
    SetupRequired,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalModelServerStatus {
    pub state: LocalModelServerState,
    pub backend: String,
    pub model: String,
    pub detail: String,
    pub server_path: Option<PathBuf>,
    pub endpoint: Option<String>,
}

impl Default for LocalModelServerStatus {
    fn default() -> Self {
        let (backend, model) = default_embedding_backend();
        Self {
            state: LocalModelServerState::SetupRequired,
            backend: backend.to_string(),
            model: model.to_string(),
            detail: "model server component has not been prepared".to_string(),
            server_path: None,
            endpoint: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BundledModelServer {
    pub path: PathBuf,
    pub sha256: String,
    pub runtime_dir: PathBuf,
    pub venv_dir: PathBuf,
    pub requirements: Vec<BundledModelServerFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BundledModelServerFile {
    pub path: PathBuf,
    pub sha256: String,
}

struct BundledFile {
    name: &'static str,
    content: &'static [u8],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelServerMetadata {
    pid: u32,
    host: String,
    port: u16,
    component_sha256: String,
    profile: String,
    started_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelSnapshotMarker {
    model: String,
    revision: String,
    snapshot_path: PathBuf,
    files: Vec<String>,
}

struct StartupLock {
    _file: File,
}

pub(crate) struct LocalModelServerEmbeddingBackend {
    rara_home: PathBuf,
    client: reqwest::Client,
    endpoint: Mutex<Option<String>>,
}

impl LocalModelServerEmbeddingBackend {
    pub(crate) fn new(rara_home: PathBuf) -> Result<Self> {
        let status = prepare_local_model_server_status(&rara_home);
        Self::from_initial_status(rara_home, status)
    }

    pub(crate) fn from_initial_status(
        rara_home: PathBuf,
        status: LocalModelServerStatus,
    ) -> Result<Self> {
        Ok(Self {
            rara_home,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .no_proxy()
                .build()
                .context("build local embedding HTTP client")?,
            endpoint: Mutex::new(status.endpoint),
        })
    }

    async fn embed_once(
        &self,
        endpoint: &str,
        text: &str,
        kind: EmbeddingInputKind,
    ) -> Result<Vec<f32>> {
        let embeddings_url = format!("{}/v1/embeddings", endpoint.trim_end_matches('/'));
        let response = self
            .client
            .post(&embeddings_url)
            .json(&serde_json::json!({
                "input": text,
                "input_type": kind.as_api_value(),
            }))
            .send()
            .await
            .with_context(|| {
                format!(
                    "send local embedding request to {}",
                    sanitize_url_for_display(&embeddings_url)
                )
            })?;
        if !response.status().is_success() {
            let body = redact_secrets(response.text().await.unwrap_or_default());
            bail!(
                "local embedding request failed at {}: {}",
                sanitize_url_for_display(&embeddings_url),
                body
            );
        }
        let payload: Value = response
            .json()
            .await
            .context("parse local embedding response")?;
        let embedding = payload
            .get("data")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("embedding"))
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("local embedding response missing data[0].embedding"))?;
        embedding
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .map(|number| number as f32)
                    .ok_or_else(|| anyhow!("local embedding vector contained non-numeric value"))
            })
            .collect()
    }

    fn cached_endpoint(&self) -> Option<String> {
        self.endpoint
            .lock()
            .expect("local embedding endpoint lock poisoned")
            .clone()
    }

    async fn refresh_endpoint(&self) -> Result<String> {
        let rara_home = self.rara_home.clone();
        let result = tokio::task::spawn_blocking(move || {
            let status = inspect_local_model_server_status(&rara_home);
            let Some(endpoint) = status.endpoint else {
                return Err(anyhow!(
                    "local embedding model server unavailable: state={:?}; {}",
                    status.state,
                    status.detail
                ));
            };
            Ok(endpoint)
        })
        .await
        .context("join local embedding endpoint refresh")?;

        match result {
            Ok(endpoint) => {
                *self
                    .endpoint
                    .lock()
                    .expect("local embedding endpoint lock poisoned") = Some(endpoint.clone());
                Ok(endpoint)
            }
            Err(err) => {
                *self
                    .endpoint
                    .lock()
                    .expect("local embedding endpoint lock poisoned") = None;
                Err(err)
            }
        }
    }
}

#[async_trait]
impl EmbeddingBackend for LocalModelServerEmbeddingBackend {
    async fn embed(&self, text: &str, kind: EmbeddingInputKind) -> Result<Vec<f32>> {
        let endpoint = match self.cached_endpoint() {
            Some(endpoint) => endpoint,
            None => self.refresh_endpoint().await?,
        };
        match self.embed_once(&endpoint, text, kind).await {
            Ok(vector) => Ok(vector),
            Err(first_error) => {
                let refreshed_endpoint = self.refresh_endpoint().await?;
                if refreshed_endpoint == endpoint {
                    return Err(first_error);
                }
                self.embed_once(&refreshed_endpoint, text, kind).await
            }
        }
    }
}

pub(crate) fn ensure_bundled_model_server(rara_home: &Path) -> Result<BundledModelServer> {
    let runtime_dir = rara_home.join("runtime").join("model-server");
    fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("create {}", runtime_dir.display()))?;
    ensure_runtime_dir_inside_home(rara_home, &runtime_dir)?;

    let path = runtime_dir.join(MODEL_SERVER_NAME);
    let expected_hash = sha256_hex(MODEL_SERVER);
    if existing_file_matches(&path, &expected_hash)? {
        return Ok(BundledModelServer {
            path,
            sha256: expected_hash,
            runtime_dir: runtime_dir.clone(),
            venv_dir: runtime_dir.join("venv"),
            requirements: ensure_model_server_requirements(&runtime_dir)?,
        });
    }

    write_file_atomically(&path, MODEL_SERVER)?;
    Ok(BundledModelServer {
        path,
        sha256: expected_hash,
        runtime_dir: runtime_dir.clone(),
        venv_dir: runtime_dir.join("venv"),
        requirements: ensure_model_server_requirements(&runtime_dir)?,
    })
}

pub(crate) fn prepare_local_model_server_status(rara_home: &Path) -> LocalModelServerStatus {
    prepare_local_model_server_status_with_progress(rara_home, None)
}

pub(crate) fn prepare_local_model_server_status_with_progress(
    rara_home: &Path,
    progress: Option<LocalProgressReporter>,
) -> LocalModelServerStatus {
    prepare_local_model_server_status_inner(rara_home, BootstrapMode::Automatic, progress)
}

pub(crate) fn inspect_local_model_server_status(rara_home: &Path) -> LocalModelServerStatus {
    prepare_local_model_server_status_inner(rara_home, BootstrapMode::InspectOnly, None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootstrapMode {
    Automatic,
    InspectOnly,
}

fn prepare_local_model_server_status_inner(
    rara_home: &Path,
    mode: BootstrapMode,
    progress: Option<LocalProgressReporter>,
) -> LocalModelServerStatus {
    let profile = default_embedding_profile();
    let backend = profile.backend;
    let model = profile.model;
    match ensure_bundled_model_server(rara_home) {
        Ok(server) => {
            if let Some(status) = reusable_server_status(&server, backend, model) {
                return status;
            }

            if mode == BootstrapMode::InspectOnly {
                return inspect_prepared_runtime_status(&server, backend, model);
            }

            let lock_path = startup_lock_path(&server.runtime_dir);
            let Ok(_lock) = StartupLock::try_acquire(&lock_path) else {
                return LocalModelServerStatus {
                    state: LocalModelServerState::WaitingForServer,
                    backend: backend.to_string(),
                    model: model.to_string(),
                    detail: "waiting for another RARA process to finish model server startup"
                        .to_string(),
                    server_path: Some(server.path),
                    endpoint: None,
                };
            };

            if let Some(status) = reusable_server_status(&server, backend, model) {
                return status;
            }

            let python = match prepared_runtime_python(&server).and_then(|python| match python {
                Some(python) => Ok(python),
                None => prepare_managed_runtime(&server),
            }) {
                Ok(python) => python,
                Err(err) => {
                    return LocalModelServerStatus {
                        state: LocalModelServerState::Error,
                        backend: backend.to_string(),
                        model: model.to_string(),
                        detail: err.to_string(),
                        server_path: Some(server.path),
                        endpoint: None,
                    };
                }
            };

            let model_path = match prepare_local_embedding_model_snapshot(
                &server.runtime_dir,
                &profile,
                &progress,
            ) {
                Ok(path) => path,
                Err(err) => {
                    return LocalModelServerStatus {
                        state: LocalModelServerState::Error,
                        backend: backend.to_string(),
                        model: model.to_string(),
                        detail: format!("failed to prepare model files: {err}"),
                        server_path: Some(server.path),
                        endpoint: None,
                    };
                }
            };

            start_model_server(&server, &python, backend, model, model_path.as_deref())
        }
        Err(err) => LocalModelServerStatus {
            state: LocalModelServerState::Error,
            backend: backend.to_string(),
            model: model.to_string(),
            detail: err.to_string(),
            server_path: None,
            endpoint: None,
        },
    }
}

fn inspect_prepared_runtime_status(
    server: &BundledModelServer,
    backend: &str,
    model: &str,
) -> LocalModelServerStatus {
    let python = venv_python_path(&server.venv_dir);
    match prepared_runtime_python(server) {
        Ok(Some(_)) => LocalModelServerStatus {
            state: LocalModelServerState::PreparedButStopped,
            backend: backend.to_string(),
            model: model.to_string(),
            detail: "managed Python runtime is prepared; model server is not running".to_string(),
            server_path: Some(server.path.clone()),
            endpoint: None,
        },
        Ok(None) => LocalModelServerStatus {
            state: LocalModelServerState::SetupRequired,
            backend: backend.to_string(),
            model: model.to_string(),
            detail: if python.is_file() {
                "managed Python venv exists but dependencies are not installed".to_string()
            } else {
                format!("venv python missing at {}", python.display())
            },
            server_path: Some(server.path.clone()),
            endpoint: None,
        },
        Err(err) => LocalModelServerStatus {
            state: LocalModelServerState::Error,
            backend: backend.to_string(),
            model: model.to_string(),
            detail: format!("failed to inspect managed Python runtime: {err}"),
            server_path: Some(server.path.clone()),
            endpoint: None,
        },
    }
}

fn prepared_runtime_python(server: &BundledModelServer) -> Result<Option<PathBuf>> {
    let python = venv_python_path(&server.venv_dir);
    if !python.is_file() {
        return Ok(None);
    }
    let requirements = selected_requirements_file(server)?;
    if !requirements_marker_matches(
        &requirements_marker_path(&server.runtime_dir),
        &requirements.sha256,
    )? {
        return Ok(None);
    }
    Ok(Some(python))
}

fn prepare_managed_runtime(server: &BundledModelServer) -> Result<PathBuf> {
    let python = ensure_managed_venv(server).context("failed to create managed Python venv")?;
    if let Err(err) = ensure_model_server_dependencies(server, &python)
        .context("failed to install model server dependencies")
    {
        if let Err(cleanup_err) = cleanup_failed_venv(server) {
            return Err(anyhow!(
                "{err}; also failed to remove incomplete managed venv {}: {cleanup_err}",
                server.venv_dir.display()
            ));
        }
        return Err(anyhow!(
            "{err}; removed incomplete managed venv {}",
            server.venv_dir.display()
        ));
    }
    Ok(python)
}

fn cleanup_failed_venv(server: &BundledModelServer) -> Result<()> {
    ensure_runtime_dir_inside_home(&server.runtime_dir, &server.venv_dir)?;
    if server.venv_dir.exists() {
        fs::remove_dir_all(&server.venv_dir)
            .with_context(|| format!("remove failed managed venv {}", server.venv_dir.display()))?;
    }
    Ok(())
}

fn reusable_server_status(
    server: &BundledModelServer,
    backend: &str,
    model: &str,
) -> Option<LocalModelServerStatus> {
    let metadata = read_server_metadata(&metadata_path(&server.runtime_dir)).ok()??;
    if metadata.component_sha256 != server.sha256 || metadata.profile != embedding_profile_id() {
        return None;
    }
    let endpoint = endpoint_url(&metadata.host, metadata.port);
    let health = probe_health(&metadata.host, metadata.port).ok()?;
    if !health_identity_matches(&health, backend, model) || !health_model_ready(&health, backend) {
        return None;
    }
    Some(LocalModelServerStatus {
        state: LocalModelServerState::Ready,
        backend: backend.to_string(),
        model: model.to_string(),
        detail: format!("reusing model server at {endpoint}"),
        server_path: Some(server.path.clone()),
        endpoint: Some(endpoint),
    })
}

fn start_model_server(
    server: &BundledModelServer,
    python: &Path,
    backend: &str,
    model: &str,
    model_path: Option<&Path>,
) -> LocalModelServerStatus {
    let host = DEFAULT_MODEL_SERVER_HOST;
    let port = DEFAULT_MODEL_SERVER_PORT;
    let endpoint = endpoint_url(host, port);
    let model_cache_dir = default_local_model_cache_dir();
    match Command::new(python)
        .arg(&server.path)
        .env("RARA_MODEL_SERVER_HOST", host)
        .env("RARA_MODEL_SERVER_PORT", port.to_string())
        .env("RARA_MODEL_CACHE_DIR", &model_cache_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => {
            let metadata = ModelServerMetadata {
                pid: child.id(),
                host: host.to_string(),
                port,
                component_sha256: server.sha256.clone(),
                profile: embedding_profile_id().to_string(),
                started_at: unix_timestamp_secs(),
            };
            if let Err(err) = write_server_metadata(&metadata_path(&server.runtime_dir), &metadata)
            {
                return LocalModelServerStatus {
                    state: LocalModelServerState::Error,
                    backend: backend.to_string(),
                    model: model.to_string(),
                    detail: format!("failed to write model server metadata: {err}"),
                    server_path: Some(server.path.clone()),
                    endpoint: Some(endpoint),
                };
            }

            for _ in 0..STARTUP_HEALTH_ATTEMPTS {
                if let Ok(health) = probe_health(host, port) {
                    if health_identity_matches(&health, backend, model) {
                        return match prepare_model(host, port, backend, model_path) {
                            Ok(()) => LocalModelServerStatus {
                                state: LocalModelServerState::Ready,
                                backend: backend.to_string(),
                                model: model.to_string(),
                                detail: format!(
                                    "started model server and prepared model at {endpoint}"
                                ),
                                server_path: Some(server.path.clone()),
                                endpoint: Some(endpoint),
                            },
                            Err(err) => LocalModelServerStatus {
                                state: LocalModelServerState::Error,
                                backend: backend.to_string(),
                                model: model.to_string(),
                                detail: format!("failed to prepare model: {err}"),
                                server_path: Some(server.path.clone()),
                                endpoint: Some(endpoint),
                            },
                        };
                    }
                }
                std::thread::sleep(STARTUP_HEALTH_DELAY);
            }

            if let Ok(()) = prepare_model(host, port, backend, model_path) {
                if let Ok(health) = probe_health(host, port) {
                    if health_identity_matches(&health, backend, model)
                        && health_model_ready(&health, backend)
                    {
                        return LocalModelServerStatus {
                            state: LocalModelServerState::Ready,
                            backend: backend.to_string(),
                            model: model.to_string(),
                            detail: format!(
                                "started model server and prepared model at {endpoint}"
                            ),
                            server_path: Some(server.path.clone()),
                            endpoint: Some(endpoint),
                        };
                    }
                }
            }

            LocalModelServerStatus {
                state: LocalModelServerState::Starting,
                backend: backend.to_string(),
                model: model.to_string(),
                detail: format!("started model server process; waiting for health at {endpoint}"),
                server_path: Some(server.path.clone()),
                endpoint: Some(endpoint),
            }
        }
        Err(err) => LocalModelServerStatus {
            state: LocalModelServerState::Error,
            backend: backend.to_string(),
            model: model.to_string(),
            detail: format!("failed to start model server: {err}"),
            server_path: Some(server.path.clone()),
            endpoint: Some(endpoint),
        },
    }
}

fn default_embedding_profile() -> LocalEmbeddingModelProfile {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        LocalEmbeddingModelProfile {
            backend: "mlx_qwen3",
            model: MLX_QWEN3_MODEL_ID,
            revision: MODEL_REVISION,
            snapshot_preparation: SnapshotPreparation::RustManaged {
                required_files: SnapshotRequiredFiles::MlxQwen3,
            },
        }
    } else {
        LocalEmbeddingModelProfile {
            backend: "fastembed_bge_m3",
            model: FASTEMBED_BGE_M3_MODEL_ID,
            revision: MODEL_REVISION,
            snapshot_preparation: SnapshotPreparation::PythonManaged,
        }
    }
}

fn default_embedding_backend() -> (&'static str, &'static str) {
    let profile = default_embedding_profile();
    (profile.backend, profile.model)
}

fn embedding_profile_id() -> &'static str {
    "qwen3-embedding-0.6b"
}

fn venv_python_path(venv_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

fn ensure_managed_venv(server: &BundledModelServer) -> Result<PathBuf> {
    let python = venv_python_path(&server.venv_dir);
    if python.is_file() {
        return Ok(python);
    }
    let python_launcher =
        std::env::var_os("RARA_PYTHON").unwrap_or_else(|| std::ffi::OsString::from("python3"));
    let output = Command::new(&python_launcher)
        .arg("-m")
        .arg("venv")
        .arg(&server.venv_dir)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run {:?} -m venv", python_launcher))?;
    ensure_command_success(output, "create managed Python venv")?;
    if !python.is_file() {
        bail!("venv python was not created at {}", python.display());
    }
    Ok(python)
}

fn ensure_model_server_dependencies(server: &BundledModelServer, python: &Path) -> Result<()> {
    let requirements = selected_requirements_file(server)?;
    let marker_path = requirements_marker_path(&server.runtime_dir);
    if requirements_marker_matches(&marker_path, &requirements.sha256)? {
        return Ok(());
    }
    let output = Command::new(python)
        .arg("-m")
        .arg("pip")
        .arg("--disable-pip-version-check")
        .arg("install")
        .arg("-r")
        .arg(&requirements.path)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run {} -m pip install", python.display()))?;
    ensure_command_success(output, "install model server dependencies")?;
    let marker = serde_json::json!({
        "requirements_sha256": requirements.sha256,
        "requirements_path": requirements.path,
    });
    write_file_atomically(&marker_path, serde_json::to_vec_pretty(&marker)?.as_slice())?;
    Ok(())
}

fn selected_requirements_file(server: &BundledModelServer) -> Result<&BundledModelServerFile> {
    let name = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        REQUIREMENTS_MACOS_ARM64_NAME
    } else {
        REQUIREMENTS_PORTABLE_NAME
    };
    server
        .requirements
        .iter()
        .find(|file| file.path.ends_with(name))
        .ok_or_else(|| anyhow!("missing bundled requirements file {name}"))
}

fn requirements_marker_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(REQUIREMENTS_MARKER_NAME)
}

fn requirements_marker_matches(path: &Path, expected_hash: &str) -> Result<bool> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
    };
    let value: Value =
        serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
    Ok(value
        .get("requirements_sha256")
        .and_then(Value::as_str)
        .is_some_and(|hash| hash == expected_hash))
}

fn prepare_local_embedding_model_snapshot(
    runtime_dir: &Path,
    profile: &LocalEmbeddingModelProfile,
    progress: &Option<LocalProgressReporter>,
) -> Result<Option<PathBuf>> {
    let SnapshotPreparation::RustManaged { required_files } = profile.snapshot_preparation else {
        return Ok(None);
    };
    let model = profile.model;

    let cache_dir = default_local_model_cache_dir();
    let marker_path = model_snapshot_marker_path(runtime_dir);
    if let Some(marker) =
        read_matching_model_snapshot_marker(&marker_path, model, profile.revision)?
    {
        if cached_snapshot_under_cache(&marker.snapshot_path, &cache_dir)?
            && snapshot_has_all_files(&marker.snapshot_path, &marker.files)
        {
            report_progress(
                progress,
                format!(
                    "Model · already available at {}",
                    marker.snapshot_path.display()
                ),
            );
            return Ok(Some(marker.snapshot_path));
        }
    }

    let repo = Repo::with_revision(
        model.to_string(),
        RepoType::Model,
        profile.revision.to_string(),
    );
    let cache = Cache::new(cache_dir.clone());
    let cache_repo = cache.repo(repo.clone());
    report_progress(
        progress,
        format!("Model · checking local snapshot for {model}"),
    );
    if let Some((snapshot_path, files)) =
        local_cached_model_snapshot(&cache_dir, &repo, required_files, profile.revision)?
    {
        write_model_snapshot_marker(
            &marker_path,
            model,
            profile.revision,
            &snapshot_path,
            &files,
        )?;
        report_progress(
            progress,
            format!("Model · already available at {}", snapshot_path.display()),
        );
        return Ok(Some(snapshot_path));
    }

    let mut builder = ApiBuilder::from_cache(cache)
        .with_progress(false)
        .with_retries(3);
    if let Ok(endpoint) = std::env::var("HF_ENDPOINT") {
        builder = builder.with_endpoint(endpoint);
    }
    if let Some(token) = std::env::var("HF_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
    {
        builder = builder.with_token(Some(token));
    }
    let api = builder.build().context("build Hugging Face API client")?;
    let api_repo = api.repo(repo);
    report_progress(
        progress,
        format!("Model · resolving model metadata for {model}"),
    );
    let info = api_repo
        .info()
        .context("resolve model repository metadata")?;
    let files: Vec<String> = info
        .siblings
        .into_iter()
        .map(|sibling| sibling.rfilename)
        .filter(|name| !name.ends_with('/'))
        .collect();
    if files.is_empty() {
        bail!("model repository has no downloadable files");
    }

    let snapshot_path = cache_repo.pointer_path(&info.sha);
    if snapshot_has_all_files(&snapshot_path, &files) {
        write_model_snapshot_marker(
            &marker_path,
            model,
            profile.revision,
            &snapshot_path,
            &files,
        )?;
        report_progress(
            progress,
            format!("Model · already available at {}", snapshot_path.display()),
        );
        return Ok(Some(snapshot_path));
    }

    report_progress(
        progress,
        format!("Model · downloading {} file(s)", files.len()),
    );
    for filename in &files {
        let target = snapshot_path.join(filename);
        if target.exists() {
            report_progress(progress, format!("Model · cached {filename}"));
            continue;
        }
        report_progress(progress, format!("Model · downloading {filename}"));
        api_repo
            .download_with_progress(
                filename,
                TuiDownloadProgress::new(filename.clone(), progress.clone()),
            )
            .with_context(|| format!("download model file {filename}"))?;
    }

    if !snapshot_has_all_files(&snapshot_path, &files) {
        bail!("model snapshot is incomplete after download");
    }
    write_model_snapshot_marker(
        &marker_path,
        model,
        profile.revision,
        &snapshot_path,
        &files,
    )?;
    report_progress(
        progress,
        format!("Model · ready at {}", snapshot_path.display()),
    );
    Ok(Some(snapshot_path))
}

fn snapshot_has_all_files(snapshot_path: &Path, files: &[String]) -> bool {
    files
        .iter()
        .all(|filename| snapshot_path.join(filename).exists())
}

fn local_cached_model_snapshot(
    cache_dir: &Path,
    repo: &Repo,
    required_files: SnapshotRequiredFiles,
    revision: &str,
) -> Result<Option<(PathBuf, Vec<String>)>> {
    let repo_dir = cache_dir.join(repo.folder_name());
    let ref_path = repo_dir.join("refs").join(revision);
    let commit_hash = match fs::read_to_string(&ref_path) {
        Ok(hash) => hash.trim().to_string(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", ref_path.display())),
    };
    if commit_hash.is_empty() {
        return Ok(None);
    }
    let snapshot_path = repo_dir.join("snapshots").join(commit_hash);
    if !cached_snapshot_under_cache(&snapshot_path, cache_dir)? {
        return Ok(None);
    }
    let files = collect_snapshot_files(&snapshot_path)?;
    if files.is_empty() || !snapshot_has_minimum_model_files(required_files, &files) {
        return Ok(None);
    }
    Ok(Some((snapshot_path, files)))
}

fn collect_snapshot_files(snapshot_path: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_snapshot_files_inner(snapshot_path, snapshot_path, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_snapshot_files_inner(
    root: &Path,
    current: &Path,
    files: &mut Vec<String>,
) -> Result<()> {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("read {}", current.display())),
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("read entry in {}", current.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("stat {}", path.display()))?;
        if file_type.is_dir() {
            collect_snapshot_files_inner(root, &path, files)?;
        } else if file_type.is_file() || file_type.is_symlink() {
            let relative = path
                .strip_prefix(root)
                .with_context(|| format!("strip {}", root.display()))?;
            files.push(relative_path_string(relative));
        }
    }
    Ok(())
}

fn relative_path_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn snapshot_has_minimum_model_files(
    required_files: SnapshotRequiredFiles,
    files: &[String],
) -> bool {
    match required_files {
        SnapshotRequiredFiles::MlxQwen3 => {
            let has_config = files.iter().any(|file| file == "config.json");
            let has_tokenizer = files
                .iter()
                .any(|file| file == "tokenizer.json" || file == "tokenizer.model");
            let has_weights = files.iter().any(|file| file.ends_with(".safetensors"));
            has_config && has_tokenizer && has_weights
        }
    }
}

fn model_snapshot_marker_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(MODEL_SNAPSHOT_MARKER_NAME)
}

fn read_matching_model_snapshot_marker(
    path: &Path,
    expected_model: &str,
    expected_revision: &str,
) -> Result<Option<ModelSnapshotMarker>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
    };
    let marker: ModelSnapshotMarker =
        serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
    if marker.model != expected_model || marker.revision != expected_revision {
        return Ok(None);
    }
    if marker.files.is_empty() {
        return Ok(None);
    }
    Ok(Some(marker))
}

fn write_model_snapshot_marker(
    path: &Path,
    model: &str,
    revision: &str,
    snapshot_path: &Path,
    files: &[String],
) -> Result<()> {
    let marker = ModelSnapshotMarker {
        model: model.to_string(),
        revision: revision.to_string(),
        snapshot_path: snapshot_path.to_path_buf(),
        files: files.to_vec(),
    };
    write_file_atomically(path, serde_json::to_vec_pretty(&marker)?.as_slice())
}

fn cached_snapshot_under_cache(snapshot_path: &Path, cache_dir: &Path) -> Result<bool> {
    let snapshot = match fs::canonicalize(snapshot_path) {
        Ok(path) => path,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(err).with_context(|| format!("resolve {}", snapshot_path.display()));
        }
    };
    let cache = match fs::canonicalize(cache_dir) {
        Ok(path) => path,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("resolve {}", cache_dir.display())),
    };
    Ok(snapshot == cache || snapshot.starts_with(cache))
}

fn report_progress(progress: &Option<LocalProgressReporter>, message: impl Into<String>) {
    if let Some(callback) = progress {
        callback(message.into());
    }
}

struct TuiDownloadProgress {
    filename: String,
    progress: Option<LocalProgressReporter>,
    total: usize,
    current: usize,
    last_percent: Option<usize>,
}

impl TuiDownloadProgress {
    fn new(filename: String, progress: Option<LocalProgressReporter>) -> Self {
        Self {
            filename,
            progress,
            total: 0,
            current: 0,
            last_percent: None,
        }
    }

    fn emit(&mut self, force: bool) {
        let percent = if self.total == 0 {
            0
        } else {
            self.current.saturating_mul(100) / self.total
        };
        if !force && self.last_percent == Some(percent) {
            return;
        }
        self.last_percent = Some(percent);
        report_progress(
            &self.progress,
            format!(
                "Model · {} · {}% ({}/{})",
                self.filename,
                percent,
                format_bytes(self.current),
                format_bytes(self.total)
            ),
        );
    }
}

impl HfProgress for TuiDownloadProgress {
    fn init(&mut self, size: usize, filename: &str) {
        self.total = size;
        self.current = 0;
        self.filename = filename.to_string();
        self.emit(true);
    }

    fn update(&mut self, size: usize) {
        self.current = self.current.saturating_add(size);
        self.emit(false);
    }

    fn finish(&mut self) {
        self.current = self.total;
        self.emit(true);
    }
}

fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.1}GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.1}MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1}KiB", value / KIB)
    } else {
        format!("{bytes}B")
    }
}

fn ensure_model_server_requirements(runtime_dir: &Path) -> Result<Vec<BundledModelServerFile>> {
    let requirements_dir = runtime_dir.join("requirements");
    fs::create_dir_all(&requirements_dir)
        .with_context(|| format!("create {}", requirements_dir.display()))?;

    [
        BundledFile {
            name: REQUIREMENTS_MACOS_ARM64_NAME,
            content: REQUIREMENTS_MACOS_ARM64,
        },
        BundledFile {
            name: REQUIREMENTS_PORTABLE_NAME,
            content: REQUIREMENTS_PORTABLE,
        },
    ]
    .into_iter()
    .map(|file| {
        let path = requirements_dir.join(file.name);
        let expected_hash = sha256_hex(file.content);
        if !existing_file_matches(&path, &expected_hash)? {
            write_file_atomically(&path, file.content)?;
        }
        Ok(BundledModelServerFile {
            path,
            sha256: expected_hash,
        })
    })
    .collect()
}

impl StartupLock {
    fn try_acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create lock directory {}", parent.display()))?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("open lock file {}", path.display()))?;
        file.try_lock_exclusive()
            .with_context(|| format!("lock file {}", path.display()))?;
        Ok(Self { _file: file })
    }
}

impl Drop for StartupLock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
    }
}

fn metadata_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(SERVER_METADATA_NAME)
}

fn startup_lock_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(STARTUP_LOCK_NAME)
}

fn endpoint_url(host: &str, port: u16) -> String {
    format!("http://{host}:{port}")
}

fn read_server_metadata(path: &Path) -> Result<Option<ModelServerMetadata>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
    };
    let metadata = serde_json::from_str(&content)
        .with_context(|| format!("parse model server metadata {}", path.display()))?;
    Ok(Some(metadata))
}

fn write_server_metadata(path: &Path, metadata: &ModelServerMetadata) -> Result<()> {
    let content = serde_json::to_vec_pretty(metadata)?;
    write_file_atomically(path, &content)
}

fn probe_health(host: &str, port: u16) -> Result<Value> {
    model_server_http_client(HEALTH_TIMEOUT)?
        .get(model_server_url(host, port, "/health")?)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .context("send model server health request")?
        .error_for_status()
        .context("model server health returned non-success status")?
        .json()
        .context("parse model server health response")
}

fn prepare_model(host: &str, port: u16, backend: &str, model_path: Option<&Path>) -> Result<()> {
    let mut body = serde_json::json!({ "backend": backend });
    if let Some(model_path) = model_path {
        body["model_path"] = Value::String(model_path.display().to_string());
    }
    let body = body.to_string();
    let response = post_json(host, port, "/models/prepare", &body, PREPARE_TIMEOUT)
        .context("call model prepare endpoint")?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        bail!("model prepare endpoint returned non-ok response");
    }
    if response.get("backend").and_then(Value::as_str) != Some(backend) {
        bail!("model prepare endpoint returned mismatched backend");
    }
    if response.get("state").and_then(Value::as_str) != Some("ready") {
        bail!("model prepare endpoint did not report ready state");
    }
    Ok(())
}

fn post_json(host: &str, port: u16, path: &str, body: &str, timeout: Duration) -> Result<Value> {
    let response = model_server_http_client(timeout)?
        .post(model_server_url(host, port, path)?)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .context("send model server POST request")?;
    if !response.status().is_success() {
        let status = response.status();
        let error = response
            .text()
            .unwrap_or_else(|_| "missing response body".to_string());
        bail!("model server POST returned non-success status {status}: {error}");
    }
    response.json().context("parse model server POST response")
}

fn model_server_http_client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .build()
        .context("build model server HTTP client")
}

fn model_server_url(host: &str, port: u16, path: &str) -> Result<reqwest::Url> {
    reqwest::Url::parse(&format!("http://{host}:{port}{path}"))
        .with_context(|| format!("build model server URL for {host}:{port}{path}"))
}

fn health_identity_matches(health: &Value, backend: &str, model: &str) -> bool {
    if health.get("ok").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    if health
        .get("default_embedding_backend")
        .and_then(Value::as_str)
        != Some(backend)
    {
        return false;
    }
    health
        .get("embeddings")
        .and_then(|value| value.get(backend))
        .and_then(|value| value.get("model"))
        .and_then(Value::as_str)
        .is_none_or(|reported| reported == model)
}

fn health_model_ready(health: &Value, backend: &str) -> bool {
    health
        .get("embeddings")
        .and_then(|value| value.get(backend))
        .and_then(|value| value.get("loaded"))
        .and_then(Value::as_bool)
        == Some(true)
}

fn ensure_command_success(output: Output, context: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    bail!(
        "{context} failed with status {}: {}{}{}",
        output.status,
        stderr.trim(),
        if stdout.trim().is_empty() { "" } else { "\n" },
        stdout.trim()
    );
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn ensure_runtime_dir_inside_home(rara_home: &Path, runtime_dir: &Path) -> Result<()> {
    let home = fs::canonicalize(rara_home)
        .with_context(|| format!("canonicalize {}", rara_home.display()))?;
    let runtime = fs::canonicalize(runtime_dir)
        .with_context(|| format!("canonicalize {}", runtime_dir.display()))?;
    if !runtime.starts_with(&home) {
        bail!(
            "refusing to install model server outside RARA home: {}",
            runtime.display()
        );
    }
    Ok(())
}

fn existing_file_matches(path: &Path, expected_hash: &str) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("stat {}", path.display())),
    };
    if metadata.file_type().is_symlink() {
        bail!(
            "refusing to overwrite symlinked model server: {}",
            path.display()
        );
    }
    if !metadata.is_file() {
        bail!(
            "refusing to overwrite non-file model server path: {}",
            path.display()
        );
    }
    let content = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(sha256_hex(&content) == expected_hash)
}

fn write_file_atomically(path: &Path, content: &[u8]) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            bail!(
                "refusing to overwrite symlinked model server: {}",
                path.display()
            );
        }
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid model server path: {}", path.display()))?;
    let write_id = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        write_id
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .with_context(|| format!("create {}", tmp_path.display()))?;
    file.write_all(content)
        .with_context(|| format!("write {}", tmp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {}", tmp_path.display()))?;
    drop(file);

    set_private_file_permissions(&tmp_path)?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("rename {} to {}", tmp_path.display(), path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("chmod 0600 {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn sha256_hex(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use hf_hub::{Repo, RepoType};

    use super::{
        BootstrapMode, LocalModelServerEmbeddingBackend, LocalModelServerState,
        ModelServerMetadata, StartupLock, cleanup_failed_venv, ensure_bundled_model_server,
        health_identity_matches, health_model_ready, local_cached_model_snapshot, metadata_path,
        model_snapshot_marker_path, prepare_local_model_server_status_inner,
        read_matching_model_snapshot_marker, requirements_marker_matches, requirements_marker_path,
        reusable_server_status, selected_requirements_file, sha256_hex, snapshot_has_all_files,
        snapshot_has_minimum_model_files, startup_lock_path, unix_timestamp_secs, venv_python_path,
        write_file_atomically, write_model_snapshot_marker, write_server_metadata,
    };
    use crate::llm::{EmbeddingBackend, EmbeddingInputKind};

    #[test]
    fn installs_bundled_model_server_under_rara_home() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = ensure_bundled_model_server(temp.path()).expect("install model server");

        assert!(server.path.starts_with(temp.path()));
        assert_eq!(
            fs::read(&server.path)
                .expect("read model server")
                .as_slice(),
            super::MODEL_SERVER
        );
        assert_eq!(server.sha256, sha256_hex(super::MODEL_SERVER));
        assert_eq!(server.runtime_dir, temp.path().join("runtime/model-server"));
        assert_eq!(
            server.venv_dir,
            temp.path().join("runtime/model-server/venv")
        );
        assert_eq!(server.requirements.len(), 2);
        assert!(
            server
                .requirements
                .iter()
                .any(|file| file.path.ends_with(super::REQUIREMENTS_MACOS_ARM64_NAME))
        );
        assert!(
            server
                .requirements
                .iter()
                .any(|file| file.path.ends_with(super::REQUIREMENTS_PORTABLE_NAME))
        );
    }

    #[test]
    fn repairs_modified_model_server() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = ensure_bundled_model_server(temp.path()).expect("install model server");
        fs::write(&server.path, b"tampered").expect("tamper model server");

        let repaired = ensure_bundled_model_server(temp.path()).expect("repair model server");

        assert_eq!(repaired.path, server.path);
        assert_eq!(
            fs::read(&repaired.path).expect("read repaired model server"),
            super::MODEL_SERVER
        );
    }

    #[test]
    fn status_installs_component_and_reports_missing_venv() {
        let temp = tempfile::tempdir().expect("tempdir");

        let status =
            prepare_local_model_server_status_inner(temp.path(), BootstrapMode::InspectOnly, None);

        assert_eq!(status.state, LocalModelServerState::SetupRequired);
        assert!(
            status
                .server_path
                .as_ref()
                .is_some_and(|path| path.is_file())
        );
        assert!(status.detail.contains("venv python missing"));
    }

    #[test]
    fn inspect_only_does_not_install_dependencies_when_venv_exists() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = ensure_bundled_model_server(temp.path()).expect("install model server");
        let python = venv_python_path(&server.venv_dir);
        fs::create_dir_all(python.parent().expect("python parent")).expect("mkdir venv bin");
        fs::write(&python, b"").expect("fake python");

        let status =
            prepare_local_model_server_status_inner(temp.path(), BootstrapMode::InspectOnly, None);

        assert_eq!(status.state, LocalModelServerState::SetupRequired);
        assert!(status.detail.contains("dependencies are not installed"));
        assert!(!requirements_marker_path(&server.runtime_dir).exists());
        assert!(!startup_lock_path(&server.runtime_dir).exists());
    }

    #[test]
    fn inspect_reports_prepared_runtime_when_server_is_stopped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = ensure_bundled_model_server(temp.path()).expect("install model server");
        let python = venv_python_path(&server.venv_dir);
        fs::create_dir_all(python.parent().expect("python parent")).expect("create venv bin");
        fs::write(&python, b"").expect("write venv python placeholder");
        let selected = selected_requirements_file(&server).expect("selected requirements");
        let marker = serde_json::json!({
            "requirements_sha256": selected.sha256,
            "requirements_path": selected.path,
        });
        write_file_atomically(
            &requirements_marker_path(&server.runtime_dir),
            serde_json::to_vec_pretty(&marker).unwrap().as_slice(),
        )
        .expect("write requirements marker");

        let status =
            prepare_local_model_server_status_inner(temp.path(), BootstrapMode::InspectOnly, None);

        assert_eq!(status.state, LocalModelServerState::PreparedButStopped);
        assert_eq!(status.endpoint, None);
        assert!(status.detail.contains("runtime is prepared"));
    }

    #[test]
    fn cleanup_failed_venv_removes_managed_venv() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = ensure_bundled_model_server(temp.path()).expect("install model server");
        fs::create_dir_all(&server.venv_dir).expect("create venv dir");
        fs::write(server.venv_dir.join("partial"), b"partial").expect("write partial file");

        cleanup_failed_venv(&server).expect("cleanup failed venv");

        assert!(!server.venv_dir.exists());
    }

    #[test]
    fn reuses_running_server_only_after_health_identity_matches() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = ensure_bundled_model_server(temp.path()).expect("install model server");
        let (backend, model) = super::default_embedding_backend();
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake health server");
        let port = listener.local_addr().expect("local addr").port();
        let metadata = ModelServerMetadata {
            pid: std::process::id(),
            host: "127.0.0.1".to_string(),
            port,
            component_sha256: server.sha256.clone(),
            profile: super::embedding_profile_id().to_string(),
            started_at: unix_timestamp_secs(),
        };
        write_server_metadata(&metadata_path(&server.runtime_dir), &metadata)
            .expect("write metadata");
        let response_model = model.to_string();
        let response_backend = backend.to_string();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept health request");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let body = format!(
                r#"{{"ok":true,"default_embedding_backend":"{response_backend}","embeddings":{{"{response_backend}":{{"model":"{response_model}","loaded":true}}}}}}"#
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write health response");
        });

        let status = reusable_server_status(&server, backend, model).expect("server reused");

        assert_eq!(status.state, LocalModelServerState::Ready);
        let expected_endpoint = format!("http://127.0.0.1:{port}");
        assert_eq!(status.endpoint.as_deref(), Some(expected_endpoint.as_str()));
        assert!(status.detail.contains("reusing model server"));
    }

    #[tokio::test]
    async fn local_embedding_backend_posts_query_input_type() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake embeddings server");
        let port = listener.local_addr().expect("local addr").port();
        let captured_body = Arc::new(Mutex::new(String::new()));
        let captured_body_writer = captured_body.clone();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept embeddings request");
            let mut request = Vec::new();
            let mut header_end = None;
            let mut content_length = 0usize;
            while header_end.is_none() {
                let mut buffer = [0_u8; 512];
                let read = stream.read(&mut buffer).expect("read request");
                request.extend_from_slice(&buffer[..read]);
                if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let end = position + 4;
                    header_end = Some(end);
                    let headers =
                        String::from_utf8(request[..position].to_vec()).expect("headers utf8");
                    content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            if name.eq_ignore_ascii_case("content-length") {
                                value.trim().parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();
                }
            }
            let header_end = header_end.expect("header end");
            while request.len() < header_end + content_length {
                let mut buffer = [0_u8; 512];
                let read = stream.read(&mut buffer).expect("read request body");
                request.extend_from_slice(&buffer[..read]);
            }
            let body = String::from_utf8(request[header_end..header_end + content_length].to_vec())
                .expect("body utf8");
            *captured_body_writer.lock().expect("captured body") = body;

            let response_body = r#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[1.0,2.0,3.0]}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write embeddings response");
        });

        let backend = LocalModelServerEmbeddingBackend {
            rara_home: std::env::temp_dir(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .no_proxy()
                .build()
                .expect("client"),
            endpoint: Mutex::new(Some(format!("http://127.0.0.1:{port}"))),
        };

        let vector = backend
            .embed("Explain vector search", EmbeddingInputKind::Query)
            .await
            .expect("embed via local model server");
        assert_eq!(vector, vec![1.0, 2.0, 3.0]);

        let body = captured_body.lock().expect("captured body").clone();
        assert!(body.contains(r#""input":"Explain vector search""#));
        assert!(body.contains(r#""input_type":"query""#));
    }

    #[tokio::test]
    async fn local_embedding_backend_refresh_does_not_prepare_missing_runtime() {
        let temp = tempfile::tempdir().expect("tempdir");
        let backend = LocalModelServerEmbeddingBackend::from_initial_status(
            temp.path().to_path_buf(),
            super::LocalModelServerStatus::default(),
        )
        .expect("backend");

        let err = backend
            .embed("Explain vector search", EmbeddingInputKind::Query)
            .await
            .expect_err("embed should fail without bootstrapping");

        assert!(
            err.to_string()
                .contains("local embedding model server unavailable")
        );
        assert!(
            !venv_python_path(&temp.path().join("runtime/model-server/venv")).is_file(),
            "embed refresh must not create or prepare the managed venv"
        );
    }

    #[test]
    fn stale_metadata_with_wrong_hash_is_not_reused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = ensure_bundled_model_server(temp.path()).expect("install model server");
        let (backend, model) = super::default_embedding_backend();
        let metadata = ModelServerMetadata {
            pid: std::process::id(),
            host: "127.0.0.1".to_string(),
            port: 9,
            component_sha256: "stale".to_string(),
            profile: super::embedding_profile_id().to_string(),
            started_at: unix_timestamp_secs(),
        };
        write_server_metadata(&metadata_path(&server.runtime_dir), &metadata)
            .expect("write metadata");

        assert!(reusable_server_status(&server, backend, model).is_none());
    }

    #[test]
    fn startup_lock_allows_only_one_owner() {
        let temp = tempfile::tempdir().expect("tempdir");
        let lock_path = startup_lock_path(temp.path());
        let _owner = StartupLock::try_acquire(&lock_path).expect("first owner");

        let err = match StartupLock::try_acquire(&lock_path) {
            Ok(_) => panic!("second owner should be blocked"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("lock file"));
    }

    #[test]
    fn health_identity_rejects_wrong_backend() {
        let health = serde_json::json!({
            "ok": true,
            "default_embedding_backend": "other",
            "embeddings": {},
        });

        assert!(!health_identity_matches(
            &health,
            "mlx_qwen3",
            super::MLX_QWEN3_MODEL_ID
        ));
    }

    #[test]
    fn health_ready_requires_loaded_model() {
        let health = serde_json::json!({
            "ok": true,
            "default_embedding_backend": "mlx_qwen3",
            "embeddings": {
                "mlx_qwen3": {
                    "model": super::MLX_QWEN3_MODEL_ID,
                    "loaded": false,
                }
            },
        });

        assert!(health_identity_matches(
            &health,
            "mlx_qwen3",
            super::MLX_QWEN3_MODEL_ID
        ));
        assert!(!health_model_ready(&health, "mlx_qwen3"));
    }

    #[test]
    fn selects_platform_requirement_manifest_and_marker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let server = ensure_bundled_model_server(temp.path()).expect("install model server");
        let selected = selected_requirements_file(&server).expect("selected requirements");
        let expected_name = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            super::REQUIREMENTS_MACOS_ARM64_NAME
        } else {
            super::REQUIREMENTS_PORTABLE_NAME
        };

        assert!(selected.path.ends_with(expected_name));
        let marker_path = requirements_marker_path(&server.runtime_dir);
        assert!(
            !requirements_marker_matches(&marker_path, &selected.sha256).expect("missing marker")
        );
        let marker = serde_json::json!({
            "requirements_sha256": selected.sha256,
            "requirements_path": selected.path,
        });
        write_file_atomically(
            &marker_path,
            serde_json::to_vec_pretty(&marker).unwrap().as_slice(),
        )
        .expect("write marker");

        assert!(
            requirements_marker_matches(&marker_path, &selected.sha256).expect("matching marker")
        );
    }

    #[test]
    fn model_snapshot_marker_requires_matching_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let snapshot_path = temp.path().join("cache").join("snapshot");
        fs::create_dir_all(snapshot_path.join("nested")).expect("mkdir snapshot");
        fs::write(snapshot_path.join("config.json"), b"{}").expect("write config");
        let files = vec![
            "config.json".to_string(),
            "nested/model.safetensors".to_string(),
        ];

        assert!(!snapshot_has_all_files(&snapshot_path, &files));

        fs::write(snapshot_path.join("nested/model.safetensors"), b"weights")
            .expect("write weights");
        assert!(snapshot_has_all_files(&snapshot_path, &files));

        let marker_path = model_snapshot_marker_path(temp.path());
        write_model_snapshot_marker(
            &marker_path,
            super::MLX_QWEN3_MODEL_ID,
            super::MODEL_REVISION,
            &snapshot_path,
            &files,
        )
        .expect("write marker");
        let marker = read_matching_model_snapshot_marker(
            &marker_path,
            super::MLX_QWEN3_MODEL_ID,
            super::MODEL_REVISION,
        )
        .expect("read marker")
        .expect("matching marker");
        assert_eq!(marker.snapshot_path, snapshot_path);
        assert_eq!(marker.files, files);
        assert!(
            read_matching_model_snapshot_marker(&marker_path, "other/model", super::MODEL_REVISION)
                .expect("read non-matching marker")
                .is_none()
        );
    }

    #[test]
    fn local_cached_model_snapshot_reuses_existing_ref_without_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = Repo::with_revision(
            super::MLX_QWEN3_MODEL_ID.to_string(),
            RepoType::Model,
            super::MODEL_REVISION.to_string(),
        );
        let repo_dir = temp.path().join(repo.folder_name());
        let commit = "cached-main-sha";
        let snapshot_path = repo_dir.join("snapshots").join(commit);
        fs::create_dir_all(snapshot_path.join("nested")).expect("mkdir snapshot");
        fs::create_dir_all(repo_dir.join("refs")).expect("mkdir refs");
        fs::write(repo_dir.join("refs").join(super::MODEL_REVISION), commit).expect("write ref");
        fs::write(snapshot_path.join("config.json"), b"{}").expect("write config");
        fs::write(snapshot_path.join("tokenizer.json"), b"{}").expect("write tokenizer");
        fs::write(snapshot_path.join("nested/model.safetensors"), b"weights")
            .expect("write weights");

        let (found_path, files) = local_cached_model_snapshot(
            temp.path(),
            &repo,
            super::SnapshotRequiredFiles::MlxQwen3,
            super::MODEL_REVISION,
        )
        .expect("local cache probe")
        .expect("snapshot found");

        assert_eq!(found_path, snapshot_path);
        assert_eq!(
            files,
            vec![
                "config.json".to_string(),
                "nested/model.safetensors".to_string(),
                "tokenizer.json".to_string()
            ]
        );
    }

    #[test]
    fn snapshot_required_files_are_profile_driven() {
        assert!(!snapshot_has_minimum_model_files(
            super::SnapshotRequiredFiles::MlxQwen3,
            &[
                "config.json".to_string(),
                "tokenizer.json".to_string(),
                "model.bin".to_string(),
            ]
        ));
        assert!(snapshot_has_minimum_model_files(
            super::SnapshotRequiredFiles::MlxQwen3,
            &[
                "config.json".to_string(),
                "tokenizer.json".to_string(),
                "model.safetensors".to_string(),
            ]
        ));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_model_server() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let runtime_dir = temp.path().join("runtime").join("model-server");
        fs::create_dir_all(&runtime_dir).expect("mkdir runtime");
        let target = temp.path().join("target.py");
        fs::write(&target, b"target").expect("write target");
        symlink(&target, runtime_dir.join(super::MODEL_SERVER_NAME)).expect("symlink server");

        let err = ensure_bundled_model_server(temp.path()).expect_err("symlink refused");
        assert!(err.to_string().contains("symlinked model server"));
    }
}
