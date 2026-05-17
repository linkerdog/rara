pub(crate) fn format_error_chain(err: &anyhow::Error) -> String {
    let mut chain = err.chain();
    let mut out = match chain.next() {
        Some(first) => first.to_string(),
        None => return String::new(),
    };
    for cause in chain {
        out.push_str("\nCaused by: ");
        out.push_str(&cause.to_string());
    }
    out
}

pub(crate) fn prepare_local_model_server_status_inner(
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
                None => prepare_managed_runtime(&server, &progress),
            }) {
                Ok(python) => python,
                Err(err) => {
                    return LocalModelServerStatus {
                        state: LocalModelServerState::Error,
                        backend: backend.to_string(),
                        model: model.to_string(),
                        detail: format_error_chain(&err),
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
                        detail: format!(
                            "failed to prepare model files:\n{}",
                            format_error_chain(&err)
                        ),
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

pub(crate) fn inspect_prepared_runtime_status(
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

pub(crate) fn prepared_runtime_python(server: &BundledModelServer) -> Result<Option<PathBuf>> {
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

pub(crate) fn prepare_managed_runtime(
    server: &BundledModelServer,
    progress: &Option<LocalProgressReporter>,
) -> Result<PathBuf> {
    let python =
        ensure_managed_venv(server, progress).context("failed to create managed Python venv")?;
    if let Err(err) = ensure_model_server_dependencies(server, &python, progress)
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

pub(crate) fn cleanup_failed_venv(server: &BundledModelServer) -> Result<()> {
    ensure_runtime_dir_inside_home(&server.runtime_dir, &server.venv_dir)?;
    if server.venv_dir.exists() {
        fs::remove_dir_all(&server.venv_dir)
            .with_context(|| format!("remove failed managed venv {}", server.venv_dir.display()))?;
    }
    Ok(())
}

pub(crate) fn reusable_server_status(
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
    if !health_identity_matches(&health, backend, model) {
        return None;
    }
    if !health_model_ready(&health, backend) {
        if let Some(error_message) = health_preparation_error(&health, backend) {
            return Some(LocalModelServerStatus {
                state: LocalModelServerState::Error,
                backend: backend.to_string(),
                model: model.to_string(),
                detail: format!(
                    "model server running at {endpoint}; model preparation failed: {error_message}"
                ),
                server_path: Some(server.path.clone()),
                endpoint: Some(endpoint),
            });
        }
        return Some(LocalModelServerStatus {
            state: LocalModelServerState::PreparingModel,
            backend: backend.to_string(),
            model: model.to_string(),
            detail: format!("model server running at {endpoint}; model is still loading"),
            server_path: Some(server.path.clone()),
            endpoint: Some(endpoint),
        });
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
