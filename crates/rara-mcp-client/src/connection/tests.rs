// Process fixtures are isolated and their assertions report the violated boundary.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use serde_json::json;

use super::*;

const SERVER: &str = r#"
import json, os, sys, time

mode, log_path = sys.argv[1:]
with open(log_path + '.pid', 'w') as pid:
    pid.write(str(os.getpid()))
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    if 'id' not in request:
        continue
    with open(log_path, 'a') as log:
        log.write(json.dumps(request) + '\n')
    if method == 'initialize':
        if mode == 'bad_handshake':
            print(json.dumps({'jsonrpc': '2.0', 'id': request['id'],
                              'error': {'code': -32603, 'message': 'secret-detail'}}), flush=True)
            continue
        result = {
            'protocolVersion': request['params']['protocolVersion'],
            'capabilities': {'tools': {}},
            'serverInfo': {'name': 'controlled-fixture', 'version': '1'},
        }
    elif method == 'tools/list':
        if mode == 'oversized_frame':
            sys.stdout.write(' ' * (2 * 1024 * 1024 + 1))
            sys.stdout.flush()
            time.sleep(300)
        cursor = request.get('params', {}).get('cursor')
        name = 'second' if cursor else 'first'
        if mode == 'duplicate':
            name = 'first'
        if mode == 'invalid':
            name = 'invalid\nname'
        tool = {'name': name, 'description': 'Scoped fixture',
                'inputSchema': {'type': 'object'}}
        result = {'tools': [tool]}
        if mode == 'cycle':
            result['tools'] = []
            result['nextCursor'] = 'repeated'
        elif mode == 'capacity':
            result['tools'] = [dict(tool, name='tool-' + str(i)) for i in range(513)]
        elif not cursor:
            result['nextCursor'] = 'page-2'
    elif method == 'tools/call':
        if mode == 'lost':
            sys.exit(0)
        result = {'content': [{'type': 'text', 'text': 'fixture result'}],
                  'structuredContent': {'name': request['params']['name'],
                                        'arguments': request['params']['arguments'],
                                        'scope': os.environ.get('FIXTURE_SCOPE')}}
        if mode == 'input_required':
            result = {'resultType': 'input_required', 'requestState': 'owner-state'}
    else:
        raise RuntimeError('unexpected request method')
    reply = json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}) + '\n'
    if mode == 'fragmented':
        for part in reply:
            sys.stdout.write(part)
            sys.stdout.flush()
    else:
        print(reply, end='', flush=True)
if mode == 'stubborn':
    time.sleep(300)
"#;

struct Fixture {
    root: tempfile::TempDir,
    mode: &'static str,
}

impl Fixture {
    fn new(mode: &'static str) -> Self {
        Self {
            root: tempfile::tempdir().expect("fixture directory"),
            mode,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("python3");
        command
            .args(["-u", "-c", SERVER, self.mode])
            .arg(self.root.path().join("requests.jsonl"))
            .current_dir(self.root.path())
            .env_clear()
            .env("FIXTURE_SCOPE", "session-a");
        command
    }

    fn requests(&self, method: &str) -> Vec<Value> {
        std::fs::read_to_string(self.root.path().join("requests.jsonl"))
            .expect("request log")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("logged request"))
            .filter(|request| request["method"] == method)
            .collect()
    }
}

#[tokio::test]
async fn complete_catalogue_reaches_a_real_child_and_shutdown_retires_it() {
    let fixture = Fixture::new("normal");
    let mut connection = StdioConnection::connect(fixture.command())
        .await
        .expect("connect fixture");
    let pid = connection.process_id().expect("child pid");
    assert_eq!(
        connection
            .tools()
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    let response = connection
        .call("second", json!({"input": 7}).as_object().unwrap().clone())
        .await
        .expect("call admitted tool");
    let CallToolResponse::Complete(result) = response else {
        panic!("fixture must complete without another request");
    };
    assert_eq!(
        result.structured_content,
        Some(json!({
            "name": "second", "arguments": {"input": 7}, "scope": "session-a"
        }))
    );
    assert_eq!(fixture.requests("tools/list").len(), 2);
    assert_eq!(fixture.requests("tools/call").len(), 1);
    connection.shutdown().await.expect("retire source");
    connection.shutdown().await.expect("repeat shutdown");
    assert!(connection.call("first", Map::new()).await.is_err());
    assert_eq!(fixture.requests("tools/call").len(), 1);
    #[cfg(target_os = "linux")]
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
}

#[tokio::test]
async fn calls_outside_the_catalogue_are_rejected_before_dispatch() {
    let fixture = Fixture::new("normal");
    let mut connection = StdioConnection::connect(fixture.command()).await.unwrap();
    let error = connection.call("foreign", Map::new()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "MCP tool is outside the admitted catalogue"
    );
    assert!(fixture.requests("tools/call").is_empty());
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn existing_discovery_reads_all_pages_and_waits_for_child_cleanup() {
    let fixture = Fixture::new("normal");
    let log = fixture.root.path().join("requests.jsonl");
    let tools = crate::list_stdio_tools(
        "python3".into(),
        ["-u", "-c", SERVER, "normal"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .chain([log.into_os_string()])
            .collect(),
        std::collections::HashMap::new(),
        Some(fixture.root.path().to_path_buf()),
    )
    .await
    .expect("complete discovery");
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(fixture.requests("tools/list").len(), 2);
    #[cfg(target_os = "linux")]
    {
        let pid = std::fs::read_to_string(fixture.root.path().join("requests.jsonl.pid")).unwrap();
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
}

#[tokio::test]
async fn a_lost_result_is_uncertain_and_never_retransmitted() {
    let fixture = Fixture::new("lost");
    let mut connection = StdioConnection::connect(fixture.command()).await.unwrap();
    let error = connection.call("first", Map::new()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "MCP call failed; execution outcome is uncertain"
    );
    connection.shutdown().await.unwrap();
    assert_eq!(fixture.requests("tools/call").len(), 1);
}

#[tokio::test]
async fn input_required_is_returned_without_sdk_resubmission() {
    let fixture = Fixture::new("input_required");
    let mut connection = StdioConnection::connect(fixture.command()).await.unwrap();
    let result = connection.call("first", Map::new()).await.unwrap();
    let CallToolResponse::InputRequired(result) = result else {
        panic!("owner must receive the input-required result");
    };
    assert_eq!(result.request_state.as_deref(), Some("owner-state"));
    connection.shutdown().await.unwrap();
    assert_eq!(fixture.requests("tools/call").len(), 1);
}

#[tokio::test]
async fn a_failed_handshake_retires_the_child_and_redacts_diagnostics() {
    let fixture = Fixture::new("bad_handshake");
    let Err(error) = StdioConnection::connect(fixture.command()).await else {
        panic!("bad handshake must fail");
    };
    assert_eq!(error.to_string(), "MCP initialization failed");
    assert!(fixture.requests("tools/list").is_empty());
    #[cfg(target_os = "linux")]
    {
        let pid = std::fs::read_to_string(fixture.root.path().join("requests.jsonl.pid")).unwrap();
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
}

#[tokio::test]
async fn shutdown_kills_and_reaps_a_child_that_ignores_eof() {
    let fixture = Fixture::new("stubborn");
    let mut connection = StdioConnection::connect(fixture.command()).await.unwrap();
    let pid = connection.process_id().unwrap();
    connection
        .shutdown()
        .await
        .expect("bounded forced retirement");
    assert!(connection.child.try_wait().unwrap().is_some());
    #[cfg(target_os = "linux")]
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
}

#[tokio::test]
async fn interrupted_retirement_never_becomes_a_successful_receipt() {
    let fixture = Fixture::new("stubborn");
    let mut connection = StdioConnection::connect(fixture.command()).await.unwrap();
    assert!(
        timeout(Duration::from_millis(30), connection.shutdown())
            .await
            .is_err()
    );
    assert_eq!(
        connection.shutdown().await.unwrap_err().to_string(),
        "MCP source cleanup is uncertain"
    );
    assert_eq!(
        connection
            .call("first", Map::new())
            .await
            .unwrap_err()
            .to_string(),
        "MCP source is closed"
    );
    assert!(fixture.requests("tools/call").is_empty());
    connection.child.kill().await.expect("fixture cleanup");
}

#[tokio::test]
async fn invalid_catalogues_never_admit_a_partial_source() {
    for (mode, expected, pages) in [
        (
            "duplicate",
            "MCP catalogue contains an invalid or duplicate tool name",
            2,
        ),
        (
            "invalid",
            "MCP catalogue contains an invalid or duplicate tool name",
            1,
        ),
        ("cycle", "MCP catalogue cursor is invalid or repeated", 2),
        ("capacity", "MCP catalogue exceeds capacity", 1),
        ("oversized_frame", "MCP catalogue request failed", 1),
    ] {
        let fixture = Fixture::new(mode);
        let result = StdioConnection::connect(fixture.command()).await;
        let Err(error) = result else {
            panic!("invalid catalogue must not be admitted: {mode}");
        };
        assert_eq!(error.to_string(), expected, "mode: {mode}");
        assert_eq!(fixture.requests("tools/list").len(), pages, "mode: {mode}");
        assert!(fixture.requests("tools/call").is_empty());
    }
}

#[tokio::test]
async fn fragmented_frames_preserve_the_catalogue_and_call_response() {
    let fixture = Fixture::new("fragmented");
    let mut connection = StdioConnection::connect(fixture.command()).await.unwrap();
    assert_eq!(connection.tools().len(), 2);
    assert!(matches!(
        connection.call("second", Map::new()).await.unwrap(),
        CallToolResponse::Complete(_)
    ));
    connection.shutdown().await.unwrap();
}
