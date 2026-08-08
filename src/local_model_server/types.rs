#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalModelServerState {
    Disabled,
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
        Self {
            state: LocalModelServerState::Disabled,
            backend: "none".to_string(),
            model: "none".to_string(),
            detail: "bundled embedding runtime is disabled; use official Mem for semantic recall"
                .to_string(),
            server_path: None,
            endpoint: None,
        }
    }
}
