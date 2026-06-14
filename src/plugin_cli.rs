use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, clap::Args)]
pub(crate) struct PluginInstallArgs {
    /// Local plugin directory to install into .rara/plugins
    pub(crate) source: PathBuf,

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
    /// Install a local Claude Code plugin directory
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
    let mut plugins = rara_plugins::discover_plugins(&plugins_dir);
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins
}

fn install_plugin_for_workspace(
    workspace_root: &Path,
    source: &Path,
    force: bool,
) -> Result<String> {
    let source = source
        .canonicalize()
        .with_context(|| format!("failed to resolve plugin source {}", source.display()))?;
    if !source.is_dir() {
        bail!("plugin source must be a directory: {}", source.display());
    }
    let plugin = rara_plugins::load_plugin(&source).with_context(|| {
        format!(
            "plugin source {} must contain .claude-plugin/plugin.json",
            source.display()
        )
    })?;
    let name = safe_plugin_name(&plugin.name)?;
    let plugins_dir = workspace_plugins_dir_for(workspace_root);
    let target = plugins_dir.join(&name);

    if target.exists() {
        let target_canonical = target
            .canonicalize()
            .with_context(|| format!("failed to resolve plugin target {}", target.display()))?;
        if target_canonical == source {
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
    copy_dir_recursive(&source, &target)?;
    Ok(name)
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
            install_plugin_for_workspace(workspace.path(), &source, false).expect("install plugin");
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

        remove_plugin_for_workspace(workspace.path(), "test-plugin").expect("remove plugin");
        assert!(list_plugins_for_workspace(workspace.path()).is_empty());
    }

    #[test]
    fn plugin_install_requires_force_to_replace() {
        let workspace = tempfile::tempdir().expect("workspace");
        let source_parent = tempfile::tempdir().expect("source parent");
        let source = source_parent.path().join("source-plugin");
        write_test_plugin(&source, "test-plugin");

        install_plugin_for_workspace(workspace.path(), &source, false).expect("first install");
        let err = install_plugin_for_workspace(workspace.path(), &source, false).unwrap_err();
        assert!(format!("{err}").contains("--force"));

        install_plugin_for_workspace(workspace.path(), &source, true).expect("force install");
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
}
