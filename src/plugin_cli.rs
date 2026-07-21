use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

#[derive(Debug, clap::Args)]
pub(crate) struct PluginInstallArgs {
    /// Local plugin directory or git URL to install into .rara/plugins
    pub(crate) source: String,

    /// Replace an existing installed plugin with the same name
    #[arg(long)]
    pub(crate) force: bool,
}

#[derive(Debug, clap::Args)]
pub(crate) struct PluginRemoveArgs {
    /// Installed plugin name to remove
    pub(crate) name: String,
}

#[derive(Debug, clap::Subcommand)]
pub(crate) enum PluginCommands {
    /// Install a local Claude Code plugin directory or git source
    Install(PluginInstallArgs),
    /// List plugins installed in this workspace
    List,
    /// Remove an installed plugin from this workspace
    Remove(PluginRemoveArgs),
}

pub(crate) fn run_plugin_command(cmd: PluginCommands) -> Result<()> {
    let workspace_root = std::env::current_dir().context("failed to read current directory")?;
    match cmd {
        PluginCommands::Install(args) => {
            let name = install_plugin_for_workspace(&workspace_root, &args.source, args.force)?;
            println!("Installed plugin {name}");
        }
        PluginCommands::List => {
            let plugins = list_plugins_for_workspace(&workspace_root);
            if plugins.is_empty() {
                println!("No plugins installed");
            } else {
                for plugin in plugins {
                    let version = plugin.version.as_deref().unwrap_or("unknown");
                    let hooks = rara_plugins::registered_hooks_for_plugin(&plugin).len();
                    if plugin.description.is_empty() {
                        println!("{} {} hooks={}", plugin.name, version, hooks);
                    } else {
                        println!(
                            "{} {} hooks={} - {}",
                            plugin.name, version, hooks, plugin.description
                        );
                    }
                    for warning in &plugin.load_warnings {
                        println!("  warning: {warning}");
                    }
                }
            }
        }
        PluginCommands::Remove(args) => {
            remove_plugin_for_workspace(&workspace_root, &args.name)?;
            println!("Removed plugin {}", args.name);
        }
    }
    Ok(())
}

fn workspace_plugins_dir_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".rara").join("plugins")
}

fn list_plugins_for_workspace(workspace_root: &Path) -> Vec<rara_plugins::Plugin> {
    let plugins_dir = workspace_plugins_dir_for(workspace_root);
    let mut plugins = rara_plugins::discover_plugins_from_source(
        &plugins_dir,
        rara_plugins::PluginSource::Project(plugins_dir.clone()),
    );
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins
}

fn install_plugin_for_workspace(
    workspace_root: &Path,
    source: &str,
    force: bool,
) -> Result<String> {
    let source = resolve_install_source(source)?;
    let plugin = rara_plugins::load_plugin_with_source(
        source.path(),
        rara_plugins::PluginSource::Cli(source.path().to_path_buf()),
    )
    .with_context(|| {
        format!(
            "plugin source {} must contain .claude-plugin/plugin.json",
            source.path().display()
        )
    })?;
    let name = safe_plugin_name(&plugin.name)?;
    let plugins_dir = workspace_plugins_dir_for(workspace_root);
    let target = plugins_dir.join(&name);

    if target.exists() {
        let target_canonical = target
            .canonicalize()
            .with_context(|| format!("failed to resolve plugin target {}", target.display()))?;
        if target_canonical == source.path() {
            bail!("plugin {name} is already installed at {}", target.display());
        }
        if !force {
            bail!("plugin {name} is already installed; pass --force to replace it");
        }
        fs::remove_dir_all(&target)
            .with_context(|| format!("failed to replace plugin {}", target.display()))?;
    }

    fs::create_dir_all(&plugins_dir).with_context(|| {
        format!(
            "failed to create plugin directory {}",
            plugins_dir.display()
        )
    })?;
    copy_dir_recursive(source.path(), &target)?;
    Ok(name)
}

struct ResolvedPluginSource {
    path: PathBuf,
    temp_root: Option<PathBuf>,
}

impl ResolvedPluginSource {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ResolvedPluginSource {
    fn drop(&mut self) {
        if let Some(temp_root) = self.temp_root.take() {
            remove_temp_plugin_checkout(&temp_root);
        }
    }
}

fn remove_temp_plugin_checkout(temp_root: &Path) {
    if let Err(err) = fs::remove_dir_all(temp_root) {
        log::warn!(
            "failed to remove temporary plugin checkout {}: {err}",
            temp_root.display()
        );
    }
}

fn resolve_install_source(source: &str) -> Result<ResolvedPluginSource> {
    if is_git_source(source) {
        return clone_git_plugin_source(source);
    }
    let path = PathBuf::from(source)
        .canonicalize()
        .with_context(|| format!("failed to resolve plugin source {source}"))?;
    if !path.is_dir() {
        bail!("plugin source must be a directory: {}", path.display());
    }
    Ok(ResolvedPluginSource {
        path,
        temp_root: None,
    })
}

fn is_git_source(source: &str) -> bool {
    source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("ssh://")
        || source.starts_with("git://")
        || source.starts_with("file://")
        || source.starts_with("git@")
}

fn clone_git_plugin_source(source: &str) -> Result<ResolvedPluginSource> {
    let temp_root = unique_plugin_checkout_dir();
    let checkout = temp_root.join("checkout");
    fs::create_dir_all(&temp_root).with_context(|| {
        format!(
            "failed to create temporary plugin checkout directory {}",
            temp_root.display()
        )
    })?;
    let source_guard = ResolvedPluginSource {
        path: checkout,
        temp_root: Some(temp_root),
    };
    let output = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg(source)
        .arg(source_guard.path())
        .output()
        .with_context(|| format!("failed to run git clone for plugin source {source}"))?;
    if !output.status.success() {
        bail!(
            "failed to clone plugin source {source}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(source_guard)
}

fn unique_plugin_checkout_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "rara-plugin-install-{}-{nanos}",
        std::process::id()
    ))
}

fn remove_plugin_for_workspace(workspace_root: &Path, name: &str) -> Result<()> {
    let name = safe_plugin_name(name)?;
    let target = workspace_plugins_dir_for(workspace_root).join(&name);
    if !target.exists() {
        bail!("plugin {name} is not installed");
    }
    fs::remove_dir_all(&target)
        .with_context(|| format!("failed to remove plugin {}", target.display()))?;
    Ok(())
}

fn safe_plugin_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains(std::path::MAIN_SEPARATOR)
    {
        bail!("invalid plugin name: {name:?}");
    }
    Ok(name.to_string())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)
        .with_context(|| format!("failed to create directory {}", target.display()))?;
    for entry in fs::read_dir(source)
        .with_context(|| format!("failed to read directory {}", source.display()))?
    {
        let entry = entry
            .with_context(|| format!("failed to read directory entry in {}", source.display()))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", source_path.display()))?;
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        } else if file_type.is_symlink() {
            bail!(
                "plugin install does not support symlinks: {}",
                source_path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_install_list_and_remove_round_trip() {
        let workspace = tempfile::tempdir().expect("workspace");
        let source_parent = tempfile::tempdir().expect("source parent");
        let source = source_parent.path().join("source-plugin");
        write_test_plugin(&source, "test-plugin");

        let name =
            install_plugin_for_workspace(workspace.path(), &source.display().to_string(), false)
                .expect("install plugin");
        assert_eq!(name, "test-plugin");
        assert!(
            workspace
                .path()
                .join(".rara/plugins/test-plugin/.claude-plugin/plugin.json")
                .exists()
        );

        let plugins = list_plugins_for_workspace(workspace.path());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "test-plugin");
        assert_eq!(plugins[0].source.label(), "project");

        remove_plugin_for_workspace(workspace.path(), "test-plugin").expect("remove plugin");
        assert!(list_plugins_for_workspace(workspace.path()).is_empty());
    }

    #[test]
    fn plugin_install_requires_force_to_replace() {
        let workspace = tempfile::tempdir().expect("workspace");
        let source_parent = tempfile::tempdir().expect("source parent");
        let source = source_parent.path().join("source-plugin");
        write_test_plugin(&source, "test-plugin");

        install_plugin_for_workspace(workspace.path(), &source.display().to_string(), false)
            .expect("first install");
        let err =
            install_plugin_for_workspace(workspace.path(), &source.display().to_string(), false)
                .unwrap_err();
        assert!(format!("{err}").contains("--force"));

        install_plugin_for_workspace(workspace.path(), &source.display().to_string(), true)
            .expect("force install");
    }

    #[test]
    fn plugin_install_accepts_git_file_url_source() {
        let workspace = tempfile::tempdir().expect("workspace");
        let repo = tempfile::tempdir().expect("repo");
        write_test_plugin(repo.path(), "git-plugin");
        run_git(repo.path(), &["init"]);
        run_git(repo.path(), &["add", "."]);
        run_git(
            repo.path(),
            &[
                "-c",
                "user.name=RARA Test",
                "-c",
                "user.email=rara@example.test",
                "commit",
                "-m",
                "add plugin",
            ],
        );
        let source = format!("file://{}", repo.path().display());

        let name = install_plugin_for_workspace(workspace.path(), &source, false)
            .expect("install git plugin");

        assert_eq!(name, "git-plugin");
        assert!(
            workspace
                .path()
                .join(".rara/plugins/git-plugin/.claude-plugin/plugin.json")
                .exists()
        );
        let plugins = list_plugins_for_workspace(workspace.path());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "git-plugin");
    }

    #[test]
    fn plugin_remove_rejects_path_like_name() {
        let workspace = tempfile::tempdir().expect("workspace");
        let err = remove_plugin_for_workspace(workspace.path(), "../plugin").unwrap_err();
        assert!(format!("{err}").contains("invalid plugin name"));
    }

    fn write_test_plugin(root: &Path, name: &str) {
        fs::create_dir_all(root.join(".claude-plugin")).expect("plugin metadata dir");
        fs::write(
            root.join(".claude-plugin/plugin.json"),
            serde_json::json!({
                "name": name,
                "version": "1.0.0",
                "description": "test plugin"
            })
            .to_string(),
        )
        .expect("plugin json");
        fs::create_dir_all(root.join("hooks")).expect("hooks dir");
        fs::write(
            root.join("hooks/hooks.json"),
            serde_json::json!({
                "Stop": [{
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "echo ok", "timeout": 1}]
                }]
            })
            .to_string(),
        )
        .expect("hooks json");
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
