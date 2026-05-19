pub(crate) fn default_embedding_profile() -> LocalEmbeddingModelProfile {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        LocalEmbeddingModelProfile {
            id: "qwen3-embedding-0.6b",
            backend: "mlx_qwen3",
            model: MLX_QWEN3_MODEL_ID,
            revision: MODEL_REVISION,
            required_files: SnapshotRequiredFiles::MlxQwen3,
        }
    } else {
        LocalEmbeddingModelProfile {
            id: "bge-m3-fastembed",
            backend: "fastembed_bge_m3",
            model: FASTEMBED_BGE_M3_MODEL_ID,
            revision: MODEL_REVISION,
            required_files: SnapshotRequiredFiles::FastEmbedBgeM3,
        }
    }
}

pub(crate) fn default_embedding_backend() -> (&'static str, &'static str) {
    let profile = default_embedding_profile();
    (profile.backend, profile.model)
}

pub(crate) fn embedding_profile_id() -> &'static str {
    default_embedding_profile().id
}

pub(crate) fn venv_python_path(venv_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

/// Find a Python >= 3.10 on the system.
///
/// Respects `RARA_PYTHON` if set; otherwise probes known executable names in
/// descending version order and picks the first one whose reported version is
/// at least 3.10.
pub(crate) fn ensure_uv_installed() -> Result<()> {
    match Command::new("uv")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => Ok(()),
        _ => bail!(
            "uv is not installed; install it:\n  https://docs.astral.sh/uv/getting-started/installation/"
        ),
    }
}

pub(crate) fn ensure_managed_venv(
    server: &BundledModelServer,
    progress: &Option<LocalProgressReporter>,
) -> Result<PathBuf> {
    let python = venv_python_path(&server.venv_dir);
    if python.is_file() {
        return Ok(python);
    }
    if server.venv_dir.exists() {
        fs::remove_dir_all(&server.venv_dir)
            .with_context(|| format!("remove stale venv {}", server.venv_dir.display()))?;
    }
    report_progress(progress, "Embedding · creating Python venv with uv");
    ensure_uv_installed()?;
    let output = Command::new("uv")
        .arg("venv")
        .arg("--python")
        .arg("3.14")
        .arg("--seed")
        .arg(&server.venv_dir)
        .stdin(Stdio::null())
        .output()
        .with_context(|| "run uv venv --python 3.14 --seed")?;
    ensure_command_success(output, "create managed Python venv with uv")?;
    if !python.is_file() {
        bail!("venv python was not created at {}", python.display());
    }
    Ok(python)
}

pub(crate) fn ensure_model_server_dependencies(
    server: &BundledModelServer,
    python: &Path,
    progress: &Option<LocalProgressReporter>,
) -> Result<()> {
    let requirements = selected_requirements_file(server)?;
    let marker_path = requirements_marker_path(&server.runtime_dir);
    if requirements_marker_matches(&marker_path, &requirements.sha256)? {
        return Ok(());
    }

    report_progress(
        progress,
        "Embedding · installing model server dependencies with uv",
    );
    let output = Command::new("uv")
        .arg("pip")
        .arg("install")
        .arg("--python")
        .arg(python)
        .arg("-r")
        .arg(&requirements.path)
        .stdin(Stdio::null())
        .output()
        .with_context(|| "run uv pip install")?;
    ensure_command_success(output, "install model server dependencies with uv")?;

    let marker = serde_json::json!({
        "requirements_sha256": requirements.sha256,
        "requirements_path": requirements.path,
    });
    write_file_atomically(&marker_path, serde_json::to_vec_pretty(&marker)?.as_slice())?;
    Ok(())
}

pub(crate) fn selected_requirements_file(
    server: &BundledModelServer,
) -> Result<&BundledModelServerFile> {
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

pub(crate) fn requirements_marker_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(REQUIREMENTS_MARKER_NAME)
}

pub(crate) fn requirements_marker_matches(path: &Path, expected_hash: &str) -> Result<bool> {
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
