use std::time::Duration;

use backon::{ExponentialBuilder, Retryable};
use rara_persistence::redaction::redact_secrets;
use serde::Serialize;
use serde_json::{Value, json};

use crate::llm::is_retryable_http_error;
use crate::tool::ToolError;

const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";
const DEFAULT_TIMEOUT_SECS: u64 = 25;

#[derive(Clone, Debug)]
pub(super) struct ExaMcpClient {
    client: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
}

impl ExaMcpClient {
    pub(super) fn from_env() -> Self {
        let api_key = std::env::var("EXA_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_string());
        Self::new(EXA_MCP_URL, api_key)
    }

    fn new(endpoint: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: endpoint.into(),
            api_key,
        }
    }

    pub(super) async fn call<T>(&self, tool: &str, arguments: &T) -> Result<String, ToolError>
    where
        T: Serialize + Sync,
    {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": tool,
                "arguments": arguments,
            },
        });
        let endpoint = self.request_endpoint()?;

        let response = (|| async {
            let res = self
                .client
                .post(endpoint.clone())
                .header("Accept", "application/json, text/event-stream")
                .json(&request)
                .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!(e))?;
            let status = res.status();
            if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(anyhow::anyhow!("HTTP {}", status.as_u16()));
            }
            Ok(res)
        })
        .retry(ExponentialBuilder::default().with_jitter())
        .when(|e: &anyhow::Error| is_retryable_http_error(e))
        .await
        .map_err(|err| {
            ToolError::ExecutionFailed(redact_secrets(format!("Exa MCP request failed: {err}")))
        })?;

        let status = response.status();
        let body = response.text().await.map_err(|err| {
            ToolError::ExecutionFailed(redact_secrets(format!("Exa MCP response failed: {err}")))
        })?;

        if !status.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "Exa MCP returned HTTP {status}: {}",
                truncate_error(&body)
            )));
        }
        if let Some(error) = parse_mcp_response_error(&body) {
            return Err(ToolError::ExecutionFailed(format!(
                "Exa MCP returned JSON-RPC error: {}",
                truncate_error(&error)
            )));
        }

        parse_mcp_response_text(&body).ok_or_else(|| {
            ToolError::ExecutionFailed("Exa MCP response did not include text content".to_string())
        })
    }

    fn request_endpoint(&self) -> Result<reqwest::Url, ToolError> {
        let mut url = reqwest::Url::parse(&self.endpoint).map_err(|err| {
            ToolError::ExecutionFailed(format!("invalid Exa MCP endpoint: {err}"))
        })?;
        if let Some(api_key) = &self.api_key {
            url.query_pairs_mut().append_pair("exaApiKey", api_key);
        }
        Ok(url)
    }
}

fn truncate_error(input: &str) -> String {
    let trimmed = input.trim();
    let mut rendered = trimmed.chars().take(500).collect::<String>();
    if rendered.chars().count() < trimmed.chars().count() {
        rendered.push_str("...");
    }
    rendered
}

pub(super) fn parse_mcp_response_text(body: &str) -> Option<String> {
    parse_json_rpc_text(body).or_else(|| parse_sse_text(body))
}

fn parse_mcp_response_error(body: &str) -> Option<String> {
    parse_json_rpc_error(body).or_else(|| parse_sse_error(body))
}

fn parse_sse_text(body: &str) -> Option<String> {
    let mut text_blocks = Vec::new();
    for line in body.lines().map(str::trim) {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Some(text) = parse_json_rpc_text(data) {
            text_blocks.push(text);
        }
    }
    (!text_blocks.is_empty()).then(|| text_blocks.join("\n"))
}

fn parse_sse_error(body: &str) -> Option<String> {
    for line in body.lines().map(str::trim) {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Some(error) = parse_json_rpc_error(data) {
            return Some(error);
        }
    }
    None
}

fn parse_json_rpc_text(input: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(input).ok()?;
    let content = value.pointer("/result/content")?.as_array()?;
    let text = content
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn parse_json_rpc_error(input: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(input).ok()?;
    let error = value.get("error")?;
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        return Some(message.to_string());
    }
    if let Some(text) = error.as_str() {
        return Some(text.to_string());
    }
    Some(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{ExaMcpClient, parse_mcp_response_error, parse_mcp_response_text};

    #[test]
    fn parses_sse_mcp_text_response() {
        let body = r#"event: message
data: {"result":{"content":[{"type":"text","text":"search result"}]}}
"#;

        assert_eq!(
            parse_mcp_response_text(body).as_deref(),
            Some("search result")
        );
    }

    #[test]
    fn concatenates_multiple_sse_text_events() {
        let body = r#"data: {"result":{"content":[{"type":"text","text":"first"}]}}
data: {"result":{"content":[{"type":"text","text":"second"}]}}
data: [DONE]
"#;

        assert_eq!(
            parse_mcp_response_text(body).as_deref(),
            Some("first\nsecond")
        );
    }

    #[test]
    fn parses_plain_json_mcp_text_response() {
        let body = r#"{"result":{"content":[{"type":"text","text":"json result"}]}}"#;

        assert_eq!(
            parse_mcp_response_text(body).as_deref(),
            Some("json result")
        );
    }

    #[test]
    fn concatenates_multiple_text_content_blocks() {
        let body = r#"{"result":{"content":[{"type":"text","text":"one"},{"type":"image","data":"ignored"},{"type":"text","text":"two"}]}}"#;

        assert_eq!(parse_mcp_response_text(body).as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn parses_json_rpc_error_message() {
        let body = r#"{"error":{"code":-32000,"message":"bad request"}}"#;

        assert_eq!(
            parse_mcp_response_error(body).as_deref(),
            Some("bad request")
        );
    }

    #[test]
    fn ignores_empty_sse_events() {
        let body = "data: [DONE]\n\n";

        assert_eq!(parse_mcp_response_text(body), None);
    }

    #[test]
    fn request_endpoint_adds_exa_key_only_when_needed() {
        let client = ExaMcpClient::new("https://mcp.exa.ai/mcp", Some("secret value".to_string()));
        let endpoint = client.request_endpoint().expect("endpoint");

        assert_eq!(endpoint.scheme(), "https");
        assert!(endpoint.as_str().contains("exaApiKey=secret+value"));
    }
}
