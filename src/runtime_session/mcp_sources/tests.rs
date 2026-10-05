use async_trait::async_trait;

mod native;
use rara_tools::tool::{Tool, ToolCallContext, ToolError};
use serde_json::{Value, json};

use super::*;

const SERVER: &str = r#"
import json, os, sys
log = sys.argv[1]
with open(log + '.pid', 'w') as out: out.write(str(os.getpid()))
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    method = request['method']
    if method == 'initialize':
        result = {'protocolVersion': request['params']['protocolVersion'], 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'owned', 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'echo', 'description': 'Owned echo', 'inputSchema': {'type': 'object'}}]}
    elif method == 'tools/call':
        with open(log, 'a') as out: out.write(json.dumps(request) + '\n')
        result = {'content': [], 'structuredContent': {'input': request['params']['arguments'], 'scope': os.environ.get('SOURCE_SCOPE'), 'cwd': os.getcwd()}}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

pub(crate) fn registration(root: &Path, source: &str) -> McpSourceRegistration {
    let python = std::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(python.status.success());
    McpSourceRegistration {
        source_id: source.into(),
        command: String::from_utf8(python.stdout).unwrap().trim().into(),
        args: vec![
            "-u".into(),
            "-c".into(),
            SERVER.into(),
            root.join(format!("{source}.jsonl"))
                .to_string_lossy()
                .into_owned(),
        ],
        env: BTreeMap::from([("SOURCE_SCOPE".into(), source.into())]),
    }
}

fn context(root: &Path, session: &str) -> ToolCallContext {
    ToolCallContext::default()
        .with_session_id(session)
        .with_turn_id("turn-1")
        .with_call_id("call-1")
        .with_workspace_root(root)
}

#[tokio::test]
async fn source_admission_enforces_context_and_retirement_fences_retained_tools() {
    let root = tempfile::tempdir().unwrap();
    let mut sources = McpSources::new(
        McpSourcePolicy::Enabled,
        "session-a".into(),
        root.path().into(),
    );
    let mut tools = ToolManager::new();
    sources
        .control(
            McpSourceControlRequest::Register(registration(root.path(), "alpha")),
            &mut tools,
        )
        .await
        .unwrap();
    let name = tool_name("alpha", "echo");
    assert!(is_controlled_mcp_tool(&name));
    let tool = tools.get_tool(&name).unwrap();
    assert!(tool.call(json!({})).await.is_err());
    assert!(
        tool.call_with_context_events(json!({}), context(root.path(), "session-b"), &mut |_| {})
            .await
            .is_err()
    );
    assert!(!root.path().join("alpha.jsonl").exists());
    let result = tool
        .call_with_context_events(
            json!({"value": 9}),
            context(root.path(), "session-a"),
            &mut |_| {},
        )
        .await
        .unwrap();
    assert_eq!(
        result["structuredContent"],
        json!({"input": {"value": 9}, "scope": "alpha", "cwd": root.path()})
    );
    // Retain the manager to prove connection admission, not only name removal,
    // prevents a previously obtained tool from executing after retirement.
    sources
        .control(
            McpSourceControlRequest::Unregister {
                source_id: "alpha".into(),
            },
            &mut ToolManager::new(),
        )
        .await
        .unwrap();
    assert!(
        tool.call_with_context_events(json!({}), context(root.path(), "session-a"), &mut |_| {})
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("alpha.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    sources.shutdown().await.unwrap();
}

struct ExistingTool(String);
#[async_trait]
impl Tool for ExistingTool {
    fn name(&self) -> &str {
        &self.0
    }
    fn description(&self) -> &str {
        "existing"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        Ok(json!("existing"))
    }
}

#[tokio::test]
async fn collisions_are_atomic_and_removal_preserves_other_sources_and_builtin_tools() {
    let root = tempfile::tempdir().unwrap();
    let mut sources = McpSources::new(
        McpSourcePolicy::Enabled,
        "session-a".into(),
        root.path().into(),
    );
    let mut tools = ToolManager::new();
    let collision = tool_name("alpha", "echo");
    tools.register(Box::new(ExistingTool(collision.clone())));
    assert!(matches!(
        sources
            .control(
                McpSourceControlRequest::Register(registration(root.path(), "alpha")),
                &mut tools
            )
            .await,
        Err(RuntimeSessionError::InvalidSource)
    ));
    assert_eq!(
        tools
            .get_tool(&collision)
            .unwrap()
            .call(json!({}))
            .await
            .unwrap(),
        json!("existing")
    );
    assert!(sources.sources.is_empty());
    for source in ["beta", "gamma"] {
        sources
            .control(
                McpSourceControlRequest::Register(registration(root.path(), source)),
                &mut tools,
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        sources
            .control(
                McpSourceControlRequest::Register(registration(root.path(), "beta")),
                &mut tools
            )
            .await,
        Err(RuntimeSessionError::InvalidSource)
    ));
    sources
        .control(
            McpSourceControlRequest::Unregister {
                source_id: "beta".into(),
            },
            &mut tools,
        )
        .await
        .unwrap();
    assert!(tools.get_tool(&tool_name("beta", "echo")).is_none());
    assert!(tools.get_tool(&tool_name("gamma", "echo")).is_some());
    assert!(tools.get_tool(&collision).is_some());
    sources.shutdown().await.unwrap();
}

#[tokio::test]
async fn disabled_or_invalid_sources_do_not_launch_a_child() {
    let root = tempfile::tempdir().unwrap();
    let mut sources = McpSources::new(
        McpSourcePolicy::Disabled,
        "session-a".into(),
        root.path().into(),
    );
    let mut tools = ToolManager::new();
    let request = registration(root.path(), "alpha");
    assert!(matches!(
        sources
            .control(
                McpSourceControlRequest::Register(request.clone()),
                &mut tools
            )
            .await,
        Err(RuntimeSessionError::UnsupportedSource)
    ));
    sources.policy = McpSourcePolicy::Enabled;
    let mut relative = request;
    relative.command = "python3".into();
    assert!(matches!(
        sources
            .control(McpSourceControlRequest::Register(relative), &mut tools)
            .await,
        Err(RuntimeSessionError::InvalidSource)
    ));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
