#[cfg(test)]
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use super::{
    BootstrapMode, LocalModelServerEmbeddingBackend, LocalModelServerState, ModelServerMetadata,
    StartupLock, cleanup_failed_venv, ensure_bundled_model_server, ensure_managed_venv,
    health_identity_matches, health_model_ready, local_cached_model_snapshot, metadata_path,
    model_snapshot_marker_path, prepare_local_model_server_status_inner,
    read_matching_model_snapshot_marker, requirements_marker_matches, requirements_marker_path,
    reusable_server_status, selected_requirements_file, selected_snapshot_files, sha256_hex,
    snapshot_has_all_files, snapshot_has_minimum_model_files, startup_lock_path,
    unix_timestamp_secs, venv_python_path, write_file_atomically, write_model_snapshot_marker,
    write_server_metadata,
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
fn bundled_model_server_does_not_start_background_model_prepare() {
    let source = std::str::from_utf8(super::MODEL_SERVER).expect("model server utf8");
    let main_body = source
        .split("def main()")
        .nth(1)
        .expect("model server main function");

    assert!(
        !main_body.contains("start_background_preparation"),
        "Rust startup must provide the local snapshot path before the Python server loads a model"
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
    write_server_metadata(&metadata_path(&server.runtime_dir), &metadata).expect("write metadata");
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
            if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
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
    write_server_metadata(&metadata_path(&server.runtime_dir), &metadata).expect("write metadata");

    assert!(reusable_server_status(&server, backend, model).is_none());
}

#[test]
fn stale_metadata_with_exited_pid_is_not_reused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let server = ensure_bundled_model_server(temp.path()).expect("install model server");
    let (backend, model) = super::default_embedding_backend();

    // Create a known-dead PID by spawning a short-lived process.
    let mut child = std::process::Command::new("true").spawn().expect("spawn");
    let dead_pid = child.id();
    child.wait().expect("wait");

    let metadata = ModelServerMetadata {
        pid: dead_pid,
        host: "127.0.0.1".to_string(),
        port: 9,
        component_sha256: server.sha256.clone(),
        profile: super::embedding_profile_id().to_string(),
        started_at: unix_timestamp_secs(),
    };
    write_server_metadata(&metadata_path(&server.runtime_dir), &metadata).expect("write metadata");

    assert!(
        reusable_server_status(&server, backend, model).is_none(),
        "metadata with exited PID should not be reused"
    );
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
    assert!(!requirements_marker_matches(&marker_path, &selected.sha256).expect("missing marker"));
    let marker = serde_json::json!({
        "requirements_sha256": selected.sha256,
        "requirements_path": selected.path,
    });
    write_file_atomically(
        &marker_path,
        serde_json::to_vec_pretty(&marker).unwrap().as_slice(),
    )
    .expect("write marker");

    assert!(requirements_marker_matches(&marker_path, &selected.sha256).expect("matching marker"));
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

    fs::write(snapshot_path.join("nested/model.safetensors"), b"weights").expect("write weights");
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
    let repo_dir = temp
        .path()
        .join(super::model_cache_folder(super::MLX_QWEN3_MODEL_ID));
    let commit = "cached-main-sha";
    let snapshot_path = repo_dir.join("snapshots").join(commit);
    fs::create_dir_all(snapshot_path.join("nested")).expect("mkdir snapshot");
    fs::create_dir_all(repo_dir.join("refs")).expect("mkdir refs");
    fs::write(repo_dir.join("refs").join(super::MODEL_REVISION), commit).expect("write ref");
    fs::write(snapshot_path.join("config.json"), b"{}").expect("write config");
    fs::write(snapshot_path.join("tokenizer.json"), b"{}").expect("write tokenizer");
    fs::write(snapshot_path.join("nested/model.safetensors"), b"weights").expect("write weights");

    let (found_path, files) = local_cached_model_snapshot(
        temp.path(),
        super::MLX_QWEN3_MODEL_ID,
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
    assert!(!snapshot_has_minimum_model_files(
        super::SnapshotRequiredFiles::FastEmbedBgeM3,
        &[
            "config.json".to_string(),
            "tokenizer.json".to_string(),
            "onnx/model.onnx".to_string(),
        ]
    ));
    assert!(snapshot_has_minimum_model_files(
        super::SnapshotRequiredFiles::FastEmbedBgeM3,
        &[
            "config.json".to_string(),
            "tokenizer.json".to_string(),
            "onnx/model.onnx".to_string(),
            "onnx/model.onnx_data".to_string(),
        ]
    ));
}

#[test]
fn fastembed_snapshot_selection_avoids_non_onnx_weights() {
    let selected = selected_snapshot_files(
        super::SnapshotRequiredFiles::FastEmbedBgeM3,
        vec![
            "config.json".to_string(),
            "tokenizer.json".to_string(),
            "tokenizer_config.json".to_string(),
            "onnx/model.onnx".to_string(),
            "onnx/model.onnx_data".to_string(),
            "pytorch_model.bin".to_string(),
            "model.safetensors".to_string(),
        ],
    );

    assert_eq!(
        selected,
        vec![
            "config.json".to_string(),
            "tokenizer.json".to_string(),
            "tokenizer_config.json".to_string(),
            "onnx/model.onnx".to_string(),
            "onnx/model.onnx_data".to_string(),
        ]
    );
}

#[test]
fn fastembed_snapshot_selection_still_requires_external_data() {
    let selected = selected_snapshot_files(
        super::SnapshotRequiredFiles::FastEmbedBgeM3,
        vec![
            "config.json".to_string(),
            "tokenizer.json".to_string(),
            "onnx/model.onnx".to_string(),
            "pytorch_model.bin".to_string(),
        ],
    );

    assert!(!snapshot_has_minimum_model_files(
        super::SnapshotRequiredFiles::FastEmbedBgeM3,
        &selected
    ));
}

#[test]
fn ensure_managed_venv_creates_and_reuses_venv_with_pip() {
    if skip_managed_venv_test() {
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let server = ensure_bundled_model_server(temp.path()).expect("install model server");

    let venv_python = match ensure_managed_venv(&server, &None) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping ensure_managed_venv test: {e}");
            return;
        }
    };

    assert!(venv_python.is_file());
    assert!(server.venv_dir.exists());

    let pip_check = std::process::Command::new(&venv_python)
        .arg("-m")
        .arg("pip")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run pip --version");
    assert!(
        pip_check.status.success(),
        "pip should be installed, stderr: {}",
        String::from_utf8_lossy(&pip_check.stderr).trim()
    );

    let reused = ensure_managed_venv(&server, &None).expect("reuse managed venv");
    assert_eq!(reused, venv_python);
}

#[test]
fn ensure_managed_venv_cleans_stale_venv_dir() {
    if skip_managed_venv_test() {
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let server = ensure_bundled_model_server(temp.path()).expect("install model server");

    fs::create_dir_all(&server.venv_dir).expect("create fake stale venv dir");
    fs::write(server.venv_dir.join("partial"), b"leftover").expect("write partial file");
    assert!(server.venv_dir.exists());

    let venv_python = match ensure_managed_venv(&server, &None) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping stale venv test: {e}");
            return;
        }
    };
    assert!(venv_python.is_file());
    assert!(!server.venv_dir.join("partial").exists());
}

fn skip_managed_venv_test() -> bool {
    if std::env::var_os("RARA_SKIP_MANAGED_VENV_TESTS").is_none() {
        return false;
    }

    eprintln!("skipping managed venv test because RARA_SKIP_MANAGED_VENV_TESTS is set");
    true
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
