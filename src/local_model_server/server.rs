
pub(crate) fn start_model_server(
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
    let stderr_log_path = server.runtime_dir.join("model-server-stderr.log");
    let stderr_file = std::fs::File::create(&stderr_log_path)
        .unwrap_or_else(|_| std::fs::File::create("/dev/null").unwrap());
    match Command::new(python)
        .arg(&server.path)
        .env("RARA_MODEL_SERVER_HOST", host)
        .env("RARA_MODEL_SERVER_PORT", port.to_string())
        .env("RARA_MODEL_CACHE_DIR", &model_cache_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()
    {
        Ok(mut child) => {
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

            let mut health_ever_passed = false;
            let mut last_prepare_error: Option<anyhow::Error> = None;
            let mut saw_model_loading = false;
            let marker_path = model_snapshot_marker_path(&server.runtime_dir);
            for _ in 0..STARTUP_HEALTH_ATTEMPTS {
                if let Ok(health) = probe_health(host, port) {
                    health_ever_passed = true;
                    if health_identity_matches(&health, backend, model) {
                        if health_model_ready(&health, backend) {
                            return ready_model_server_status(server, backend, model, &endpoint);
                        }
                        if let Some(error_message) = health_preparation_error(&health, backend) {
                            if let Err(e) = std::fs::remove_file(&marker_path) {
                                eprintln!("warn: failed to remove model snapshot marker: {e}");
                            }
                            return LocalModelServerStatus {
                                state: LocalModelServerState::Error,
                                backend: backend.to_string(),
                                model: model.to_string(),
                                detail: format!(
                                    "model server running at {endpoint}; model preparation failed: {error_message} (stderr log: {})\nstderr tail:\n{}",
                                    stderr_log_path.display(),
                                    stderr_tail(&stderr_log_path)
                                ),
                                server_path: Some(server.path.clone()),
                                endpoint: Some(endpoint),
                            };
                        }
                        match prepare_model(host, port, backend, model_path) {
                            Ok(state) => {
                                if state == "ready" {
                                    return ready_model_server_status(
                                        server, backend, model, &endpoint,
                                    );
                                } else {
                                    saw_model_loading = true;
                                }
                            }
                            Err(err) => {
                                last_prepare_error = Some(err);
                            }
                        }
                    }
                }
                std::thread::sleep(STARTUP_HEALTH_DELAY);
                if let Ok(Some(status)) = child.try_wait() {
                    return LocalModelServerStatus {
                        state: LocalModelServerState::Error,
                        backend: backend.to_string(),
                        model: model.to_string(),
                        detail: format!(
                            "model server process exited with {status} during startup (stderr log: {})\nstderr tail:\n{}",
                            stderr_log_path.display(),
                            stderr_tail(&stderr_log_path)
                        ),
                        server_path: Some(server.path.clone()),
                        endpoint: Some(endpoint),
                    };
                }
            }
            if saw_model_loading {
                return preparing_model_server_status(
                    server,
                    backend,
                    model,
                    &endpoint,
                    &stderr_log_path,
                );
            }
            if let Some(err) = last_prepare_error {
                if let Err(e) = std::fs::remove_file(&marker_path) {
                    eprintln!("warn: failed to remove model snapshot marker: {e}");
                }
                return LocalModelServerStatus {
                    state: LocalModelServerState::Error,
                    backend: backend.to_string(),
                    model: model.to_string(),
                    detail: format!(
                        "failed to prepare model at {endpoint} after {} health checks: {err} (stderr log: {})\nstderr tail:\n{}",
                        STARTUP_HEALTH_ATTEMPTS,
                        stderr_log_path.display(),
                        stderr_tail(&stderr_log_path)
                    ),
                    server_path: Some(server.path.clone()),
                    endpoint: Some(endpoint),
                };
            }

            let stderr_hint = format!(" (stderr log: {})", stderr_log_path.display());
            LocalModelServerStatus {
                state: LocalModelServerState::Starting,
                backend: backend.to_string(),
                model: model.to_string(),
                detail: if health_ever_passed {
                    format!(
                        "started model server process; waiting for health at {endpoint}{stderr_hint}"
                    )
                } else {
                    format!(
                        "started model server process; health never reached at {endpoint}{stderr_hint}"
                    )
                },
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

/// Read the last 4 lines of the stderr log for diagnostic inclusion.
fn stderr_tail(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

fn ready_model_server_status(
    server: &BundledModelServer,
    backend: &str,
    model: &str,
    endpoint: &str,
) -> LocalModelServerStatus {
    LocalModelServerStatus {
        state: LocalModelServerState::Ready,
        backend: backend.to_string(),
        model: model.to_string(),
        detail: format!("started model server and prepared model at {endpoint}"),
        server_path: Some(server.path.clone()),
        endpoint: Some(endpoint.to_string()),
    }
}

fn preparing_model_server_status(
    server: &BundledModelServer,
    backend: &str,
    model: &str,
    endpoint: &str,
    stderr_log_path: &Path,
) -> LocalModelServerStatus {
    LocalModelServerStatus {
        state: LocalModelServerState::PreparingModel,
        backend: backend.to_string(),
        model: model.to_string(),
        detail: format!(
            "model files are available; waiting for model server at {endpoint} to load model (stderr log: {})",
            stderr_log_path.display()
        ),
        server_path: Some(server.path.clone()),
        endpoint: Some(endpoint.to_string()),
    }
}
