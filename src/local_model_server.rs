use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

const MODEL_SERVER: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/model_server/rara_model_server.py"
));
const MODEL_SERVER_NAME: &str = "rara_model_server.py";

const REQUIREMENTS_MACOS_ARM64: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/model_server/requirements-macos-arm64.txt"
));
const REQUIREMENTS_MACOS_ARM64_NAME: &str = "requirements-macos-arm64.txt";

const REQUIREMENTS_PORTABLE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/model_server/requirements-portable.txt"
));
const REQUIREMENTS_PORTABLE_NAME: &str = "requirements-portable.txt";

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

struct BundledFile {
    name: &'static str,
    content: &'static [u8],
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

fn ensure_model_server_requirements(runtime_dir: &Path) -> Result<Vec<BundledModelServerFile>> {
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

fn ensure_runtime_dir_inside_home(rara_home: &Path, runtime_dir: &Path) -> Result<()> {
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

fn existing_file_matches(path: &Path, expected_hash: &str) -> Result<bool> {
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

fn write_file_atomically(path: &Path, content: &[u8]) -> Result<()> {
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
    let tmp_path = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
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
fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("chmod 0600 {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn sha256_hex(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{ensure_bundled_model_server, sha256_hex};

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
}
