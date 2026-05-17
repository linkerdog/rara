pub(crate) const MODEL_SERVER: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/components/model_server/rara_model_server.py"
));
pub(crate) const MODEL_SERVER_NAME: &str = "rara_model_server.py";

pub(crate) const REQUIREMENTS_MACOS_ARM64: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/components/model_server/requirements-macos-arm64.txt"
));
pub(crate) const REQUIREMENTS_MACOS_ARM64_NAME: &str = "requirements-macos-arm64.txt";

pub(crate) const REQUIREMENTS_PORTABLE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/components/model_server/requirements-portable.txt"
));
pub(crate) const REQUIREMENTS_PORTABLE_NAME: &str = "requirements-portable.txt";
pub(crate) const MLX_QWEN3_MODEL_ID: &str = "mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ";
pub(crate) const FASTEMBED_BGE_M3_MODEL_ID: &str = "BAAI/bge-m3";
pub(crate) const DEFAULT_MODEL_SERVER_HOST: &str = "127.0.0.1";
pub(crate) const DEFAULT_MODEL_SERVER_PORT: u16 = 18181;
pub(crate) const SERVER_METADATA_NAME: &str = "server.json";
pub(crate) const STARTUP_LOCK_NAME: &str = "startup.lock";
pub(crate) const REQUIREMENTS_MARKER_NAME: &str = "requirements-installed.json";
pub(crate) const MODEL_SNAPSHOT_MARKER_NAME: &str = "model-snapshot.json";
pub(crate) const HEALTH_TIMEOUT: Duration = Duration::from_millis(300);
pub(crate) const PREPARE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const STARTUP_HEALTH_ATTEMPTS: usize = 30;
pub(crate) const STARTUP_HEALTH_DELAY: Duration = Duration::from_millis(200);
pub(crate) const MODEL_REVISION: &str = "main";
pub(crate) static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotPreparation {
    RustManaged {
        required_files: SnapshotRequiredFiles,
    },
    PythonManaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotRequiredFiles {
    MlxQwen3,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalEmbeddingModelProfile {
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

pub(crate) struct BundledFile {
    name: &'static str,
    content: &'static [u8],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModelServerMetadata {
    pid: u32,
    host: String,
    port: u16,
    component_sha256: String,
    profile: String,
    started_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModelSnapshotMarker {
    model: String,
    revision: String,
    snapshot_path: PathBuf,
    files: Vec<String>,
}

pub(crate) struct StartupLock {
    _file: File,
}

pub(crate) struct LocalModelServerEmbeddingBackend {
    rara_home: PathBuf,
    client: reqwest::Client,
    endpoint: Mutex<Option<String>>,
}
