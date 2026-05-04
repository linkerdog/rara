use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use backon::{ExponentialBuilder, Retryable};
use futures::StreamExt;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};
use url::Url;

use crate::llm::is_retryable_http_error;
use crate::tool::{Tool, ToolError};

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;
const HARD_MAX_BYTES: u64 = 10 * 1024 * 1024;

pub struct WebFetchTool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FetchFormat {
    Markdown,
    Text,
    Html,
}

impl FetchFormat {
    fn parse(input: Option<&str>) -> Result<Self, ToolError> {
        match input.unwrap_or("markdown").trim() {
            "markdown" => Ok(Self::Markdown),
            "text" => Ok(Self::Text),
            "html" => Ok(Self::Html),
            value => Err(ToolError::InvalidInput(format!(
                "format must be one of markdown, text, html; got {value}"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Html => "html",
        }
    }
}

#[derive(Debug)]
struct FetchRequest {
    url: Url,
    format: FetchFormat,
    timeout_secs: u64,
    max_bytes: u64,
}

#[tool_spec(
    name = "web_fetch",
    description = "Fetch a URL as markdown, text, or HTML with timeout and size limits",
    input_schema = {
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "HTTP or HTTPS URL to fetch."
            },
            "format": {
                "type": "string",
                "enum": ["markdown", "text", "html"],
                "default": "markdown",
                "description": "Output format."
            },
            "timeout_seconds": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_TIMEOUT_SECS,
                "default": DEFAULT_TIMEOUT_SECS
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1024,
                "maximum": HARD_MAX_BYTES,
                "default": DEFAULT_MAX_BYTES
            }
        },
        "required": ["url"]
    }
)]
#[async_trait]
impl Tool for WebFetchTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let request = FetchRequest::parse(&input)?;
        let client = reqwest::Client::builder()
            .redirect(public_redirect_policy())
            .timeout(Duration::from_secs(request.timeout_secs))
            .build()
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
        let response = (|| async {
            let res = client
                .get(request.url.clone())
                .header("User-Agent", "RARA/0.1.0")
                .header("Accept", accept_header(request.format))
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
        .map_err(|err| ToolError::ExecutionFailed(format!("fetch failed: {err}")))?;
        validate_public_web_url(response.url())?;

        let status = response.status();
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = read_limited_body(response, request.max_bytes).await?;
        let body_text = String::from_utf8_lossy(&body.bytes).to_string();
        let content = convert_content(&body_text, request.format);

        Ok(json!({
            "url": request.url.as_str(),
            "final_url": final_url,
            "status": status.as_u16(),
            "content_type": content_type,
            "bytes": body.bytes_read,
            "truncated": body.truncated,
            "format": request.format.as_str(),
            "content": content,
        }))
    }
}

impl FetchRequest {
    fn parse(input: &Value) -> Result<Self, ToolError> {
        let raw_url = input
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ToolError::InvalidInput("url".to_string()))?;
        let url = Url::parse(raw_url)
            .map_err(|err| ToolError::InvalidInput(format!("invalid url: {err}")))?;
        validate_public_web_url(&url)?;
        let format = FetchFormat::parse(input.get("format").and_then(Value::as_str))?;
        let timeout_secs = input
            .get("timeout_seconds")
            .or_else(|| input.get("timeout"))
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);
        let max_bytes = input
            .get("max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MAX_BYTES)
            .clamp(1024, HARD_MAX_BYTES);

        Ok(Self {
            url,
            format,
            timeout_secs,
            max_bytes,
        })
    }
}

struct LimitedBody {
    bytes: Vec<u8>,
    bytes_read: usize,
    truncated: bool,
}

async fn read_limited_body(
    response: reqwest::Response,
    max_bytes: u64,
) -> Result<LimitedBody, ToolError> {
    let limit = max_bytes as usize;
    let mut bytes = Vec::new();
    let mut truncated = response
        .content_length()
        .is_some_and(|length| length > max_bytes);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
        let remaining = limit.saturating_sub(bytes.len());
        if remaining == 0 {
            truncated = true;
            break;
        }
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    let bytes_read = bytes.len();
    Ok(LimitedBody {
        bytes,
        bytes_read,
        truncated,
    })
}

fn accept_header(format: FetchFormat) -> &'static str {
    match format {
        FetchFormat::Markdown | FetchFormat::Text => {
            "text/html, text/plain, application/xhtml+xml, */*;q=0.8"
        }
        FetchFormat::Html => "text/html, application/xhtml+xml, */*;q=0.8",
    }
}

fn convert_content(input: &str, format: FetchFormat) -> String {
    match format {
        FetchFormat::Html => input.to_string(),
        FetchFormat::Text | FetchFormat::Markdown => html_to_text(input),
    }
}

fn html_to_text(input: &str) -> String {
    let input = strip_non_text_blocks(input);
    let mut output = String::new();
    let mut in_tag = false;
    let mut last_was_space = false;
    for ch in input.chars() {
        match ch {
            '<' => {
                in_tag = true;
                push_space(&mut output, &mut last_was_space);
            }
            '>' => in_tag = false,
            _ if in_tag => {}
            _ if ch.is_whitespace() => push_space(&mut output, &mut last_was_space),
            _ => {
                output.push(ch);
                last_was_space = false;
            }
        }
    }
    decode_basic_entities(output.trim()).to_string()
}

fn validate_public_web_url(url: &Url) -> Result<(), ToolError> {
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(ToolError::InvalidInput(format!(
                "url scheme must be http or https; got {scheme}"
            )));
        }
    }

    let host = url
        .host_str()
        .ok_or_else(|| ToolError::InvalidInput("url host is required".to_string()))?;
    let normalized_host = host.trim_end_matches('.');
    if normalized_host.eq_ignore_ascii_case("localhost") {
        return Err(ToolError::InvalidInput(
            "url host must not be localhost".to_string(),
        ));
    }
    if let Ok(ip) = normalized_host.parse::<IpAddr>() {
        validate_public_ip(ip)?;
    }
    Ok(())
}

fn public_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > 10 {
            return attempt.error("too many redirects");
        }
        match validate_public_web_url(attempt.url()) {
            Ok(()) => attempt.follow(),
            Err(err) => attempt.error(format!("blocked redirect target: {err}")),
        }
    })
}

fn validate_public_ip(ip: IpAddr) -> Result<(), ToolError> {
    let blocked = match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
        }
    };
    if blocked {
        return Err(ToolError::InvalidInput(
            "url host must not be a private, loopback, link-local, documentation, or unspecified IP address"
                .to_string(),
        ));
    }
    Ok(())
}

fn strip_non_text_blocks(input: &str) -> String {
    let mut output = String::new();
    let mut remaining = input;
    loop {
        let lower = remaining.to_ascii_lowercase();
        let script_pos = lower.find("<script");
        let style_pos = lower.find("<style");
        let Some(start) = min_present(script_pos, style_pos) else {
            output.push_str(remaining);
            break;
        };
        output.push_str(&remaining[..start]);
        let tag_name = if Some(start) == script_pos {
            "script"
        } else {
            "style"
        };
        let close = format!("</{tag_name}>");
        if let Some(end) = lower[start..].find(&close) {
            remaining = &remaining[start + end + close.len()..];
        } else {
            break;
        }
    }
    output
}

fn min_present(left: Option<usize>, right: Option<usize>) -> Option<usize> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn push_space(output: &mut String, last_was_space: &mut bool) {
    if !*last_was_space && !output.is_empty() {
        output.push(' ');
        *last_was_space = true;
    }
}

fn decode_basic_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{FetchFormat, FetchRequest, html_to_text};

    #[test]
    fn fetch_request_rejects_non_http_urls() {
        let err = FetchRequest::parse(&json!({ "url": "file:///etc/passwd" }))
            .expect_err("invalid scheme");

        assert!(err.to_string().contains("url scheme must be http or https"));
    }

    #[test]
    fn fetch_request_rejects_localhost_and_private_ip_literals() {
        let localhost = FetchRequest::parse(&json!({ "url": "http://localhost:8080" }))
            .expect_err("localhost rejected");
        let localhost_trailing_dot = FetchRequest::parse(&json!({ "url": "http://localhost." }))
            .expect_err("localhost with trailing dot rejected");
        let private_ip = FetchRequest::parse(&json!({ "url": "http://169.254.169.254" }))
            .expect_err("link-local rejected");

        assert!(localhost.to_string().contains("localhost"));
        assert!(localhost_trailing_dot.to_string().contains("localhost"));
        assert!(private_ip.to_string().contains("private"));
    }

    #[test]
    fn fetch_request_applies_limits_and_defaults() {
        let request = FetchRequest::parse(&json!({
            "url": "https://example.com",
            "timeout_seconds": 999,
            "max_bytes": 1
        }))
        .expect("request");

        assert_eq!(request.format, FetchFormat::Markdown);
        assert_eq!(request.timeout_secs, 120);
        assert_eq!(request.max_bytes, 1024);
    }

    #[test]
    fn html_to_text_strips_tags_and_decodes_basic_entities() {
        let text = html_to_text("<h1>RARA &amp; Search</h1><p>hello&nbsp;world</p>");

        assert_eq!(text, "RARA & Search hello world");
    }

    #[test]
    fn html_to_text_skips_script_and_style_content() {
        let text = html_to_text(
            "<style>.hidden{display:none}</style><h1>RARA</h1><script>alert('x')</script><p>Search</p>",
        );

        assert_eq!(text, "RARA Search");
    }
}
