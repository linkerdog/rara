// Fixture failures should stop the test at the violated protocol invariant.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::*;

#[derive(Clone, Copy)]
enum ResponseMode {
    Pages,
    StallInitialize,
    StallSecondPage,
    RejectSecondPage,
}

struct Request {
    headers: String,
    body: Value,
}

struct MockServer {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    task: JoinHandle<()>,
}

impl MockServer {
    async fn start(mode: ResponseMode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert_ne!(count, 0, "request ended before its headers");
                    bytes.extend_from_slice(&chunk[..count]);
                    assert!(bytes.len() < 65536, "oversized fixture request");
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                let length = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                assert!(length < 65536, "oversized fixture body");
                while bytes.len() < header_end + length {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert_ne!(count, 0, "request ended before its body");
                    bytes.extend_from_slice(&chunk[..count]);
                }
                if !headers.starts_with("POST ") {
                    socket
                        .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await
                        .unwrap();
                    continue;
                }
                let body: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                let method = body["method"].as_str().unwrap();
                let second_page = body["params"]["cursor"] == "next";
                recorded.lock().unwrap().push(Request {
                    headers,
                    body: body.clone(),
                });
                if matches!(mode, ResponseMode::StallInitialize) && method == "initialize"
                    || matches!(mode, ResponseMode::StallSecondPage) && second_page
                {
                    std::future::pending::<()>().await;
                }
                if matches!(mode, ResponseMode::StallSecondPage) && method == "tools/list" {
                    // A per-page timeout would exceed the outer test deadline.
                    tokio::time::sleep(Duration::from_secs(6)).await;
                }
                let response = match method {
                    "initialize" => json!({
                        "jsonrpc": "2.0", "id": body["id"],
                        "result": {
                            "protocolVersion": body["params"]["protocolVersion"],
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "fixture", "version": "1"}
                        }
                    }),
                    "notifications/initialized" => {
                        socket.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                        continue;
                    }
                    "tools/list"
                        if second_page && matches!(mode, ResponseMode::RejectSecondPage) =>
                    {
                        json!({"jsonrpc": "2.0", "id": body["id"], "error": {"code": -32603, "message": "fixture rejected page"}})
                    }
                    "tools/list" => {
                        let name = if second_page { "second" } else { "first" };
                        let mut result = json!({"tools": [{"name": name, "description": "fixture tool", "inputSchema": {"type": "object"}}]});
                        if !second_page {
                            result["nextCursor"] = json!("next");
                        }
                        json!({"jsonrpc": "2.0", "id": body["id"], "result": result})
                    }
                    other => panic!("unexpected MCP method: {other}"),
                };
                let body = response.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn http_lists_all_pages_with_headers_on_every_request() {
    let server = MockServer::start(ResponseMode::Pages).await;
    let tools = list_http_tools(
        server.url.clone(),
        vec![
            ("authorization".into(), "Bearer fixture-token".into()),
            ("x-test".into(), "fixture-value".into()),
        ],
        HttpProxyPolicy::Bypass,
    )
    .await
    .unwrap();
    assert_eq!(
        tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(tools[0].description, "fixture tool");
    assert_eq!(tools[0].input_schema, json!({"type": "object"}));
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].body["method"], "initialize");
    assert_eq!(requests[1].body["method"], "notifications/initialized");
    assert!(requests[2].body["params"]["cursor"].is_null());
    assert_eq!(requests[3].body["params"]["cursor"], "next");
    for request in requests.iter() {
        let headers = request.headers.to_ascii_lowercase();
        assert!(headers.contains("authorization: bearer fixture-token\r\n"));
        assert!(headers.contains("x-test: fixture-value\r\n"));
    }
}

#[tokio::test]
async fn http_bounds_initialization() {
    let server = MockServer::start(ResponseMode::StallInitialize).await;
    let error = timeout(
        CONNECT_TIMEOUT + Duration::from_secs(3),
        list_http_tools(server.url.clone(), vec![], HttpProxyPolicy::Bypass),
    )
    .await
    .expect("connection deadline did not fire")
    .unwrap_err();
    assert_eq!(error.to_string(), "MCP connect timed out");
}

#[tokio::test]
async fn http_bounds_listing_after_a_successful_page() {
    let server = MockServer::start(ResponseMode::StallSecondPage).await;
    let error = timeout(
        LIST_TIMEOUT + Duration::from_secs(3),
        list_http_tools(server.url.clone(), vec![], HttpProxyPolicy::Bypass),
    )
    .await
    .expect("listing deadline did not fire")
    .unwrap_err();
    assert_eq!(error.to_string(), "MCP tools/list timed out");
    assert_eq!(
        server.requests.lock().unwrap().last().unwrap().body["params"]["cursor"],
        "next"
    );
}

#[tokio::test]
async fn http_rejects_partial_listing_after_page_error() {
    let server = MockServer::start(ResponseMode::RejectSecondPage).await;
    let error = list_http_tools(server.url.clone(), vec![], HttpProxyPolicy::Bypass)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "MCP tools/list failed");
    assert!(format!("{error:#}").contains("fixture rejected page"));
}

#[tokio::test]
async fn http_connection_context_omits_endpoint_credentials() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = list_http_tools(
        format!("http://{address}/mcp?token=fixture-secret"),
        vec![],
        HttpProxyPolicy::Bypass,
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "Failed to connect to MCP HTTP server");
}

#[tokio::test]
async fn http_proxy_policy_is_applied_before_connect() {
    const CHILD_URL: &str = "MCP_PROXY_TEST_URL";
    const CHILD_POLICY: &str = "MCP_PROXY_TEST_POLICY";
    if let Ok(url) = std::env::var(CHILD_URL) {
        let policy = match std::env::var(CHILD_POLICY).unwrap().as_str() {
            "system" => HttpProxyPolicy::System,
            "bypass" => HttpProxyPolicy::Bypass,
            other => panic!("unexpected fixture policy: {other}"),
        };
        let tools = list_http_tools(
            url,
            vec![("authorization".into(), "Bearer fixture-token".into())],
            policy,
        )
        .await
        .unwrap();
        assert_eq!(tools.len(), 2);
        return;
    }

    let server = MockServer::start(ResponseMode::Pages).await;
    let proxy = MockServer::start(ResponseMode::Pages).await;
    for policy in ["bypass", "system"] {
        // Keep process-global proxy variables isolated from parallel tests.
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "http_tests::http_proxy_policy_is_applied_before_connect",
                "--nocapture",
            ])
            .env(CHILD_URL, &server.url)
            .env(CHILD_POLICY, policy)
            .kill_on_drop(true);
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            child.env(name, &proxy.url);
        }
        for name in ["NO_PROXY", "no_proxy"] {
            child.env_remove(name);
        }
        let result = timeout(Duration::from_secs(25), child.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            result.status.success(),
            "child failed: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        if policy == "bypass" {
            assert_eq!(server.requests.lock().unwrap().len(), 4);
            assert!(proxy.requests.lock().unwrap().is_empty());
        } else {
            assert_eq!(server.requests.lock().unwrap().len(), 4);
            assert_eq!(proxy.requests.lock().unwrap().len(), 4);
        }
    }
}
