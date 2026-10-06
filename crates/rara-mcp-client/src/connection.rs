use std::collections::BTreeSet;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use rmcp::model::{CallToolRequestParams, CallToolResponse, PaginatedRequestParams, Tool};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Map, Value};
use tokio::process::{Child, Command};
use tokio::time::timeout;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PAGES: usize = 16;
const MAX_TOOLS: usize = 512;
const MAX_BYTES: usize = 1024 * 1024;

/// One explicitly configured child connection and its immutable admitted catalogue.
///
/// The caller owns command/environment policy and the session that may use these tools.
/// Registration never discovers other servers, refreshes implicitly, or retries a call.
pub struct StdioConnection {
    service: RunningService<RoleClient, ()>,
    child: Child,
    tools: Vec<Tool>,
    process_id: Option<u32>,
    shutdown_state: ShutdownState,
}

enum ShutdownState {
    Open,
    Closed,
    Uncertain,
}

impl StdioConnection {
    /// Connect and validate every page before exposing any tool to the caller.
    pub async fn connect(mut command: Command) -> Result<Self> {
        command.kill_on_drop(true);
        // A source may print credentials in diagnostics. Keep its stderr out of the
        // controller's protocol/log stream; failures below expose categories only.
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| anyhow!("MCP child could not start"))?;
        let process_id = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("MCP child stdout is unavailable"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("MCP child stdin is unavailable"))?;
        let stdout = crate::bounded_reader::BoundedReader::new(stdout);
        let initialized = timeout(CONNECT_TIMEOUT, ().serve((stdout, stdin)))
            .await
            .map_err(|_| anyhow!("MCP initialization timed out"))
            .and_then(|result| result.map_err(|_| anyhow!("MCP initialization failed")));
        let service = match initialized {
            Ok(service) => service,
            Err(error) => {
                retire_child(&mut child).await?;
                return Err(error);
            }
        };
        let mut connection = Self {
            service,
            child,
            tools: Vec::new(),
            process_id,
            shutdown_state: ShutdownState::Open,
        };
        let catalogue = timeout(CONNECT_TIMEOUT, connection.read_catalogue())
            .await
            .map_err(|_| anyhow!("MCP catalogue timed out"))
            .and_then(|result| result);
        match catalogue {
            Ok(tools) => {
                connection.tools = tools;
                Ok(connection)
            }
            Err(error) => {
                connection.shutdown().await?;
                Err(error)
            }
        }
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    pub fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    /// Issue exactly one request. Transport errors/timeouts leave execution uncertain.
    /// Input-required responses are returned to the owner without SDK-driven resubmission.
    pub async fn call(
        &self,
        name: &str,
        arguments: Map<String, Value>,
    ) -> Result<CallToolResponse> {
        if !matches!(self.shutdown_state, ShutdownState::Open) {
            bail!("MCP source is closed");
        }
        if !self.tools.iter().any(|tool| tool.name == name) {
            bail!("MCP tool is outside the admitted catalogue");
        }
        if serde_json::to_vec(&arguments)?.len() > MAX_BYTES {
            bail!("MCP call arguments exceed the byte limit");
        }
        let request = CallToolRequestParams::new(name.to_owned()).with_arguments(arguments);
        timeout(CALL_TIMEOUT, self.service.call_tool_once(request))
            .await
            .map_err(|_| anyhow!("MCP call timed out; execution outcome is uncertain"))?
            .map_err(|_| anyhow!("MCP call failed; execution outcome is uncertain"))
    }

    /// Close the transport and wait for the child cleanup, with a bounded deadline.
    pub async fn shutdown(&mut self) -> Result<()> {
        match self.shutdown_state {
            ShutdownState::Closed => return Ok(()),
            ShutdownState::Uncertain => bail!("MCP source cleanup is uncertain"),
            ShutdownState::Open => {}
        }
        // Closing consumes the SDK task handle, even on timeout. A second close
        // must not turn an interrupted or failed retirement into a successful one.
        self.shutdown_state = ShutdownState::Uncertain;
        let closed = self.service.close_with_timeout(CLOSE_TIMEOUT).await;
        retire_child(&mut self.child).await?;
        match closed {
            Ok(Some(_)) => {
                self.shutdown_state = ShutdownState::Closed;
                Ok(())
            }
            Ok(None) => Err(anyhow!("MCP transport cleanup timed out")),
            Err(_) => Err(anyhow!("MCP transport cleanup failed")),
        }
    }

    async fn read_catalogue(&self) -> Result<Vec<Tool>> {
        let mut tools = Vec::new();
        let mut names = BTreeSet::new();
        let mut cursors = BTreeSet::new();
        let mut cursor = None;
        let mut bytes = 0;
        for _ in 0..MAX_PAGES {
            let page = self
                .service
                .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
                .await
                .map_err(|_| anyhow!("MCP catalogue request failed"))?;
            for tool in page.tools {
                let name = tool.name.as_ref();
                if name.trim().is_empty()
                    || name.len() > 128
                    || name.chars().any(char::is_control)
                    || !names.insert(name.to_owned())
                {
                    bail!("MCP catalogue contains an invalid or duplicate tool name");
                }
                bytes += serde_json::to_vec(&tool)?.len();
                if tools.len() == MAX_TOOLS || bytes > MAX_BYTES {
                    bail!("MCP catalogue exceeds capacity");
                }
                tools.push(tool);
            }
            match page.next_cursor {
                None => return Ok(tools),
                Some(next) if next.len() <= 4096 && cursors.insert(next.clone()) => {
                    cursor = Some(next);
                }
                Some(_) => bail!("MCP catalogue cursor is invalid or repeated"),
            }
        }
        bail!("MCP catalogue exceeds the page limit")
    }
}

async fn retire_child(child: &mut Child) -> Result<()> {
    match timeout(Duration::from_secs(3), child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err(anyhow!("MCP child cleanup failed")),
        Err(_) => timeout(CLOSE_TIMEOUT, child.kill())
            .await
            .map_err(|_| anyhow!("MCP child cleanup timed out"))?
            .map_err(|_| anyhow!("MCP child cleanup failed")),
    }
}

#[cfg(all(test, unix))]
mod tests;
