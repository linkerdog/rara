pub(crate) fn ensure_model_server_requirements(
    runtime_dir: &Path,
) -> Result<Vec<BundledModelServerFile>> {
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

pub(crate) fn metadata_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(SERVER_METADATA_NAME)
}

pub(crate) fn startup_lock_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(STARTUP_LOCK_NAME)
}

pub(crate) fn endpoint_url(host: &str, port: u16) -> String {
    format!("http://{host}:{port}")
}

pub(crate) fn read_server_metadata(path: &Path) -> Result<Option<ModelServerMetadata>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
    };
    let metadata = serde_json::from_str(&content)
        .with_context(|| format!("parse model server metadata {}", path.display()))?;
    Ok(Some(metadata))
}

pub(crate) fn write_server_metadata(path: &Path, metadata: &ModelServerMetadata) -> Result<()> {
    let content = serde_json::to_vec_pretty(metadata)?;
    write_file_atomically(path, &content)
}

pub(crate) fn probe_health(host: &str, port: u16) -> Result<Value> {
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

pub(crate) fn prepare_model(
    host: &str,
    port: u16,
    backend: &str,
    model_path: Option<&Path>,
) -> Result<String> {
    let mut body = serde_json::json!({ "backend": backend });
    if let Some(model_path) = model_path {
        body["model_path"] = Value::String(model_path.display().to_string());
    }
    let body = body.to_string();
    let url = format!("http://{host}:{port}/models/prepare");
    let response = post_json(host, port, "/models/prepare", &body, PREPARE_TIMEOUT)
        .with_context(|| format!("call POST {url}"))?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        bail!("model prepare endpoint returned non-ok response");
    }
    if response.get("backend").and_then(Value::as_str) != Some(backend) {
        bail!("model prepare endpoint returned mismatched backend");
    }
    let state = response
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    if state != "ready" && state != "loading" {
        bail!("model prepare endpoint returned unexpected state: {state}");
    }
    Ok(state)
}

pub(crate) fn post_json(
    host: &str,
    port: u16,
    path: &str,
    body: &str,
    timeout: Duration,
) -> Result<Value> {
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

pub(crate) fn model_server_http_client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .build()
        .context("build model server HTTP client")
}

pub(crate) fn model_server_url(host: &str, port: u16, path: &str) -> Result<reqwest::Url> {
    reqwest::Url::parse(&format!("http://{host}:{port}{path}"))
        .with_context(|| format!("build model server URL for {host}:{port}{path}"))
}

pub(crate) fn health_identity_matches(health: &Value, backend: &str, model: &str) -> bool {
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

pub(crate) fn health_model_ready(health: &Value, backend: &str) -> bool {
    health
        .get("embeddings")
        .and_then(|value| value.get(backend))
        .and_then(|value| value.get("loaded"))
        .and_then(Value::as_bool)
        == Some(true)
}

pub(crate) fn health_preparation_error<'a>(health: &'a Value, backend: &str) -> Option<&'a str> {
    let prep = health
        .get("preparation")
        .and_then(|value| value.get(backend))?;
    if prep.get("state").and_then(Value::as_str) == Some("error") {
        prep.get("message").and_then(Value::as_str)
    } else {
        None
    }
}

pub(crate) fn ensure_command_success(output: Output, context: &str) -> Result<()> {
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

pub(crate) fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn ensure_runtime_dir_inside_home(rara_home: &Path, runtime_dir: &Path) -> Result<()> {
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

pub(crate) fn existing_file_matches(path: &Path, expected_hash: &str) -> Result<bool> {
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

pub(crate) fn write_file_atomically(path: &Path, content: &[u8]) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        bail!(
            "refusing to overwrite symlinked model server: {}",
            path.display()
        );
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
pub(crate) fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("chmod 0600 {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

/// Check whether a process identified by `pid` has exited.
/// Returns `true` if the pid is no longer running (or never existed).
#[cfg(unix)]
pub(crate) fn process_exited(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    // Signal 0 (null signal) — does not actually send a signal, only
    // checks whether the process exists and we have permission.
    kill(Pid::from_raw(pid as i32), None).is_err()
}

/// Check whether a process identified by `pid` has exited.
/// Returns `true` if the pid is no longer running (or never existed).
#[cfg(windows)]
pub(crate) fn process_exited(pid: u32) -> bool {
    let pid_text = pid.to_string();
    let filter = format!("PID eq {pid}");
    match Command::new("tasklist")
        .args(["/FI", filter.as_str(), "/NH"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            !stdout.lines().any(|line| {
                line.split_whitespace()
                    .nth(1)
                    .is_some_and(|value| value == pid_text)
            })
        }
        Ok(output) => {
            log::warn!(
                "failed to inspect Windows process {pid}: tasklist exited with {}",
                output.status
            );
            false
        }
        Err(err) => {
            log::warn!("failed to inspect Windows process {pid}: {err}");
            false
        }
    }
}

/// Check whether a process identified by `pid` has exited.
/// Returns `true` if the pid is no longer running (or never existed).
#[cfg(not(any(unix, windows)))]
pub(crate) fn process_exited(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
pub(crate) fn terminate_process(pid: u32) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    if let Err(err) = kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
        log::warn!("failed to terminate local model server process {pid}: {err}");
    }
}

#[cfg(windows)]
pub(crate) fn terminate_process(pid: u32) {
    match Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T"])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => {
            log::warn!(
                "failed to terminate local model server process {pid}: taskkill exited with {status}"
            );
        }
        Err(err) => {
            log::warn!("failed to terminate local model server process {pid}: {err}");
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn terminate_process(pid: u32) {
    log::warn!("cannot terminate local model server process {pid}: unsupported platform");
}
