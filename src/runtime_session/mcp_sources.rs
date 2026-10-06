use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use rara_mcp_client::StdioConnection;
use rara_tools::tool::ToolManager;
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::sync::Mutex;

use super::RuntimeSessionError;
use super::mcp_tool::McpSourceTool;
use crate::runtime_control::{
    McpEvent, McpSourceControlRequest, McpSourceRegistration, McpSourceSnapshot,
};

const MAX_SOURCES: usize = 16;
const MAX_TOOLS: usize = 512;
const MAX_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum McpSourcePolicy {
    #[default]
    Disabled,
    Enabled,
}

struct Source {
    tool_names: Vec<String>,
    connection: Arc<Mutex<StdioConnection>>,
    active: Arc<AtomicBool>,
}

pub(super) struct McpSources {
    policy: McpSourcePolicy,
    session_id: String,
    workspace: PathBuf,
    sources: BTreeMap<String, Source>,
    unavailable: bool,
}

impl McpSources {
    pub(super) fn new(policy: McpSourcePolicy, session_id: String, workspace: PathBuf) -> Self {
        Self {
            policy,
            session_id,
            workspace,
            sources: BTreeMap::new(),
            unavailable: false,
        }
    }

    pub(super) fn ensure_available(&self) -> Result<(), RuntimeSessionError> {
        if self.unavailable {
            Err(RuntimeSessionError::SourceUnavailable)
        } else {
            Ok(())
        }
    }

    pub(super) async fn control(
        &mut self,
        request: McpSourceControlRequest,
        tools: &mut ToolManager,
    ) -> Result<McpEvent, RuntimeSessionError> {
        self.ensure_available()?;
        if self.policy == McpSourcePolicy::Disabled {
            return Err(RuntimeSessionError::UnsupportedSource);
        }
        match request {
            McpSourceControlRequest::Register(registration) => {
                self.register(registration, tools).await
            }
            McpSourceControlRequest::Unregister { source_id } => {
                let source = self
                    .sources
                    .get(&source_id)
                    .ok_or(RuntimeSessionError::InvalidSource)?;
                source.active.store(false, Ordering::SeqCst);
                tools.retain(|name| !source.tool_names.iter().any(|owned| owned == name));
                if source.connection.lock().await.shutdown().await.is_err() {
                    self.unavailable = true;
                    return Err(RuntimeSessionError::SourceUnavailable);
                }
                self.sources.remove(&source_id);
                Ok(McpEvent::SourceUnregistered { source_id })
            }
            McpSourceControlRequest::QuerySources => Ok(McpEvent::SourcesListed {
                sources: self
                    .sources
                    .iter()
                    .map(|(source_id, source)| McpSourceSnapshot {
                        source_id: source_id.clone(),
                        tool_names: source.tool_names.clone(),
                    })
                    .collect(),
            }),
        }
    }

    async fn register(
        &mut self,
        registration: McpSourceRegistration,
        tools: &mut ToolManager,
    ) -> Result<McpEvent, RuntimeSessionError> {
        validate_registration(&registration)?;
        if self.sources.contains_key(&registration.source_id) {
            return Err(RuntimeSessionError::InvalidSource);
        }
        if self.sources.len() == MAX_SOURCES {
            return Err(RuntimeSessionError::SourceCapacity);
        }
        let mut command = Command::new(&registration.command);
        command
            .args(&registration.args)
            .env_clear()
            .envs(&registration.env)
            .current_dir(&self.workspace);
        let mut connection = match StdioConnection::connect(command).await {
            Ok(connection) => connection,
            Err(_) => {
                // A failed source operation must not become evidence that no child
                // was admitted. The stdio owner treats this as a fatal uncertainty.
                self.unavailable = true;
                return Err(RuntimeSessionError::SourceUnavailable);
            }
        };
        let names: Vec<_> = connection
            .tools()
            .iter()
            .map(|tool| tool_name(&registration.source_id, &tool.name))
            .collect();
        let total = self
            .sources
            .values()
            .map(|source| source.tool_names.len())
            .sum::<usize>()
            + names.len();
        let rejection = if total > MAX_TOOLS {
            Some(RuntimeSessionError::SourceCapacity)
        } else if names.iter().collect::<BTreeSet<_>>().len() != names.len()
            || names.iter().any(|name| tools.get_tool(name).is_some())
        {
            Some(RuntimeSessionError::InvalidSource)
        } else {
            None
        };
        if let Some(error) = rejection {
            if connection.shutdown().await.is_err() {
                self.unavailable = true;
                return Err(RuntimeSessionError::SourceUnavailable);
            }
            return Err(error);
        }
        let definitions = connection.tools().to_vec();
        let connection = Arc::new(Mutex::new(connection));
        let active = Arc::new(AtomicBool::new(true));
        for (name, definition) in names.iter().zip(definitions) {
            tools.register(Box::new(McpSourceTool {
                name: name.clone(),
                definition,
                session_id: self.session_id.clone(),
                workspace: self.workspace.clone(),
                connection: connection.clone(),
                active: active.clone(),
            }));
        }
        self.sources.insert(
            registration.source_id.clone(),
            Source {
                tool_names: names.clone(),
                connection,
                active,
            },
        );
        Ok(McpEvent::SourceRegistered {
            source_id: registration.source_id,
            tool_names: names,
        })
    }

    pub(super) async fn shutdown(&mut self) -> Result<(), RuntimeSessionError> {
        self.policy = McpSourcePolicy::Disabled;
        for source in self.sources.values() {
            source.active.store(false, Ordering::SeqCst);
        }
        for source in self.sources.values() {
            if source.connection.lock().await.shutdown().await.is_err() {
                self.unavailable = true;
            }
        }
        self.ensure_available()?;
        self.sources.clear();
        Ok(())
    }
}

fn validate_registration(registration: &McpSourceRegistration) -> Result<(), RuntimeSessionError> {
    let valid_id = !registration.source_id.is_empty()
        && registration.source_id.len() <= 128
        && registration
            .source_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte));
    let valid_env = registration.env.len() <= 64
        && registration.env.iter().all(|(key, value)| {
            !key.is_empty()
                && key.len() <= 256
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                && !value.contains('\0')
        });
    let bytes = serde_json::to_vec(registration)
        .map_err(|_| RuntimeSessionError::InvalidSource)?
        .len();
    if !valid_id
        || !valid_env
        || !Path::new(&registration.command).is_absolute()
        || registration.command.contains('\0')
        || registration.args.len() > 128
        || registration.args.iter().any(|arg| arg.contains('\0'))
    {
        return Err(RuntimeSessionError::InvalidSource);
    }
    if bytes > MAX_CONFIG_BYTES {
        return Err(RuntimeSessionError::SourceCapacity);
    }
    Ok(())
}

fn tool_name(source_id: &str, original: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(source_id.as_bytes());
    digest.update([0]);
    digest.update(original.as_bytes());
    let hex: String = digest.finalize()[..30]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("mcp_{hex}")
}

/// Controlled names reserve this shape so unknown side effects stay out of read-only modes.
pub(crate) fn is_controlled_mcp_tool(name: &str) -> bool {
    name.len() == 64
        && name
            .strip_prefix("mcp_")
            .is_some_and(|hash| hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(all(test, unix))]
pub(crate) mod tests;
