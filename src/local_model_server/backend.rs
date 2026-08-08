pub(crate) fn inspect_local_model_server_status(_rara_home: &Path) -> LocalModelServerStatus {
    LocalModelServerStatus::default()
}

/// Inspect the local model server status from a synchronous context that may itself be running
/// inside a Tokio runtime.
pub(crate) fn inspect_local_model_server_status_off_runtime(
    rara_home: &Path,
) -> LocalModelServerStatus {
    inspect_local_model_server_status(rara_home)
}
