
impl LocalModelServerEmbeddingBackend {
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

impl Drop for LocalModelServerEmbeddingBackend {
    fn drop(&mut self) {
        let runtime_dir = self.rara_home.join("runtime").join("model-server");
        if let Ok(Some(metadata)) = read_server_metadata(&metadata_path(&runtime_dir)) {
            terminate_process(metadata.pid);
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

pub(crate) fn prepare_local_model_server_status_with_progress(
    rara_home: &Path,
    progress: Option<LocalProgressReporter>,
) -> LocalModelServerStatus {
    prepare_local_model_server_status_inner(rara_home, BootstrapMode::Automatic, progress)
}

pub(crate) fn inspect_local_model_server_status(rara_home: &Path) -> LocalModelServerStatus {
    prepare_local_model_server_status_inner(rara_home, BootstrapMode::InspectOnly, None)
}

/// Inspect the local model server status from a synchronous context that may itself be running
/// inside a Tokio runtime.
///
/// `inspect_local_model_server_status` uses a `reqwest::blocking` client, which spins up and drops
/// its own Tokio runtime. Dropping a runtime while inside the async context of a multi-threaded
/// runtime panics ("Cannot drop a runtime in a context where blocking is not allowed"). When called
/// from inside a runtime we therefore hop onto a dedicated OS thread that has no runtime entered;
/// outside a runtime we probe directly.
pub(crate) fn inspect_local_model_server_status_off_runtime(
    rara_home: &Path,
) -> LocalModelServerStatus {
    if tokio::runtime::Handle::try_current().is_ok() {
        let rara_home = rara_home.to_path_buf();
        std::thread::scope(|scope| {
            scope
                .spawn(|| inspect_local_model_server_status(&rara_home))
                .join()
                .expect("local model server status probe thread panicked")
        })
    } else {
        inspect_local_model_server_status(rara_home)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BootstrapMode {
    Automatic,
    InspectOnly,
}
