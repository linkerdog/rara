pub(crate) fn prepare_local_embedding_model_snapshot(
    runtime_dir: &Path,
    profile: &LocalEmbeddingModelProfile,
    progress: &Option<LocalProgressReporter>,
) -> Result<Option<PathBuf>> {
    let required_files = profile.required_files;
    let model = profile.model;

    let cache_dir = default_local_model_cache_dir();
    let marker_path = model_snapshot_marker_path(runtime_dir);
    if let Some(marker) = read_matching_model_snapshot_marker(&marker_path, model, profile.revision)?
        && cached_snapshot_under_cache(&marker.snapshot_path, &cache_dir)?
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

    let repo_folder = model_cache_folder(model);
    report_progress(
        progress,
        format!("Model · checking local snapshot for {model}"),
    );
    if let Some((snapshot_path, files)) =
        local_cached_model_snapshot(&cache_dir, model, required_files, profile.revision)?
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

    let api = build_hf_api(&cache_dir)?;
    let api_repo = model_repo(&api, model);
    report_progress(
        progress,
        format!("Model · resolving model metadata for {model}"),
    );
    let info = api_repo
        .info()
        .revision(profile.revision)
        .send()
        .context("resolve model repository metadata")?;
    let available_files: Vec<String> = info
        .siblings
        .unwrap_or_default()
        .into_iter()
        .map(|sibling| sibling.rfilename)
        .filter(|name| !name.ends_with('/'))
        .collect();
    let files = selected_snapshot_files(required_files, available_files);
    if !snapshot_has_minimum_model_files(required_files, &files) {
        bail!("model repository is missing required files for profile");
    }

    let revision_sha = info
        .sha
        .ok_or_else(|| anyhow!("model repository metadata is missing commit SHA"))?;
    let snapshot_path = cache_dir.join(&repo_folder).join("snapshots").join(revision_sha);
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
            .download_file()
            .filename(filename)
            .revision(profile.revision)
            .progress(TuiDownloadProgress::new(filename.clone(), progress.clone()))
            .send()
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

pub(crate) fn snapshot_has_all_files(snapshot_path: &Path, files: &[String]) -> bool {
    files
        .iter()
        .all(|filename| snapshot_path.join(filename).exists())
}

pub(crate) fn selected_snapshot_files(
    required_files: SnapshotRequiredFiles,
    available_files: Vec<String>,
) -> Vec<String> {
    match required_files {
        SnapshotRequiredFiles::MlxQwen3 => available_files,
        SnapshotRequiredFiles::FastEmbedBgeM3 => available_files
            .into_iter()
            .filter(|file| {
                matches!(
                    file.as_str(),
                    "config.json"
                        | "tokenizer.json"
                        | "tokenizer_config.json"
                        | "special_tokens_map.json"
                        | "preprocessor_config.json"
                        | "onnx/model.onnx"
                        | "onnx/model.onnx_data"
                )
            })
            .collect(),
    }
}

pub(crate) fn local_cached_model_snapshot(
    cache_dir: &Path,
    model: &str,
    required_files: SnapshotRequiredFiles,
    revision: &str,
) -> Result<Option<(PathBuf, Vec<String>)>> {
    let repo_dir = cache_dir.join(model_cache_folder(model));
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

pub(crate) fn build_hf_api(cache_dir: &Path) -> Result<HFClientSync> {
    HFClient::builder()
        .cache_dir(cache_dir.to_path_buf())
        .retry_max_attempts(3)
        .build_sync()
        .context("build Hugging Face API client")
}

pub(crate) fn model_repo(api: &HFClientSync, model: &str) -> HFRepositorySync<RepoTypeModel> {
    let (owner, name) = split_id(model);
    api.model(owner, name)
}

pub(crate) fn model_cache_folder(model: &str) -> String {
    format!("models--{}", model.replace('/', "--"))
}

pub(crate) fn collect_snapshot_files(snapshot_path: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_snapshot_files_inner(snapshot_path, snapshot_path, &mut files)?;
    files.sort();
    Ok(files)
}

pub(crate) fn collect_snapshot_files_inner(
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

pub(crate) fn relative_path_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn snapshot_has_minimum_model_files(
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
        SnapshotRequiredFiles::FastEmbedBgeM3 => {
            let has_config = files.iter().any(|file| file == "config.json");
            let has_tokenizer = files.iter().any(|file| file == "tokenizer.json");
            let has_model = files.iter().any(|file| file == "onnx/model.onnx");
            let has_external_data = files.iter().any(|file| file == "onnx/model.onnx_data");
            has_config && has_tokenizer && has_model && has_external_data
        }
    }
}

pub(crate) fn model_snapshot_marker_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(MODEL_SNAPSHOT_MARKER_NAME)
}

pub(crate) fn read_matching_model_snapshot_marker(
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

pub(crate) fn write_model_snapshot_marker(
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

pub(crate) fn cached_snapshot_under_cache(snapshot_path: &Path, cache_dir: &Path) -> Result<bool> {
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
