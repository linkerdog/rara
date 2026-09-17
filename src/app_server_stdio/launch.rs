use std::path::PathBuf;

use super::server::{EVENT_CAPACITY, handshake, serve};
use crate::config::{RaraConfig, ensure_rara_home_dir};
use crate::runtime_session::{RuntimeHost, RuntimeSessionBuilder};

pub(crate) struct LaunchOptions {
    pub config: RaraConfig,
    pub workspace: PathBuf,
    pub plugin_dirs: Vec<PathBuf>,
    pub extension_discovery: bool,
    pub memory_facilities: bool,
    pub full_access: bool,
}

pub(crate) async fn run(options: LaunchOptions) -> anyhow::Result<()> {
    let workspace = options.workspace.canonicalize()?;
    anyhow::ensure!(
        workspace.is_dir(),
        "app-server workspace must be a directory"
    );
    let state_root = ensure_rara_home_dir()?;
    let mut hello = handshake();
    hello.provider = safe_label(options.config.provider.clone());
    hello.model = options.config.model.clone().and_then(safe_label);
    serve(
        std::io::stdin(),
        std::io::stdout(),
        RuntimeHost::new(),
        hello,
        move || {
            let mut config = options.config.clone();
            if options.full_access {
                config.sandbox_workspace_write.network_access = true;
            }
            let mut builder = RuntimeSessionBuilder::new(config, &workspace)
                .with_plugin_dirs(options.plugin_dirs.clone())
                .with_state_root(state_root.clone())
                .with_event_capacity(EVENT_CAPACITY);
            if !options.extension_discovery {
                builder = builder.without_extension_discovery();
            }
            if !options.memory_facilities {
                builder = builder.without_memory_facilities();
            }
            let full_access = options.full_access;
            async move {
                let session = builder.build().await?;
                if full_access && let Err(error) = session.set_full_access_mode(true).await {
                    session.shutdown().await?;
                    return Err(error.into());
                }
                Ok(session)
            }
        },
    )
    .await?;
    Ok(())
}

fn safe_label(value: String) -> Option<String> {
    let value = rara_persistence::redaction::redact_secrets(value);
    (!value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control))
        .then_some(value)
}
