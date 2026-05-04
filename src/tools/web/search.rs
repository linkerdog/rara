use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde::Serialize;
use serde_json::{Value, json};

use super::exa::ExaMcpClient;
use crate::tool::{Tool, ToolError};

const DEFAULT_NUM_RESULTS: u64 = 8;
const DEFAULT_SEARCH_TYPE: &str = "auto";
const DEFAULT_LIVECRAWL: &str = "fallback";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExaSearchArgs {
    query: String,
    #[serde(rename = "type")]
    search_type: String,
    num_results: u64,
    livecrawl: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_max_characters: Option<u64>,
}

pub struct WebSearchTool {
    client: ExaMcpClient,
}

impl WebSearchTool {
    pub fn from_env() -> Self {
        Self {
            client: ExaMcpClient::from_env(),
        }
    }
}

#[tool_spec(
    name = "web_search",
    description = "Search the web using Exa MCP for current or external information",
    input_schema = {
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query."
            },
            "num_results": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "default": DEFAULT_NUM_RESULTS,
                "description": "Maximum number of search results to return."
            },
            "livecrawl": {
                "type": "string",
                "enum": ["fallback", "preferred"],
                "default": DEFAULT_LIVECRAWL,
                "description": "Whether Exa should live-crawl result pages when cached content is missing or preferred."
            },
            "type": {
                "type": "string",
                "enum": ["auto", "fast", "deep"],
                "default": DEFAULT_SEARCH_TYPE,
                "description": "Exa search depth."
            },
            "context_max_characters": {
                "type": "integer",
                "minimum": 1000,
                "maximum": 100000,
                "description": "Optional maximum result context size."
            }
        },
        "required": ["query"]
    }
)]
#[async_trait]
impl Tool for WebSearchTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let args = parse_search_args(&input)?;
        let query = args.query.clone();
        let output = self.client.call("web_search_exa", &args).await?;
        Ok(json!({
            "query": query,
            "content": output,
            "provider": "exa_mcp",
        }))
    }
}

fn parse_search_args(input: &Value) -> Result<ExaSearchArgs, ToolError> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError::InvalidInput("query".to_string()))?
        .to_string();
    let num_results = input
        .get("num_results")
        .or_else(|| input.get("numResults"))
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_NUM_RESULTS)
        .clamp(1, 20);
    let livecrawl = enum_string(
        input,
        "livecrawl",
        &["fallback", "preferred"],
        DEFAULT_LIVECRAWL,
    )?;
    let search_type = enum_string(
        input,
        "type",
        &["auto", "fast", "deep"],
        DEFAULT_SEARCH_TYPE,
    )?;
    let context_max_characters = input
        .get("context_max_characters")
        .or_else(|| input.get("contextMaxCharacters"))
        .and_then(Value::as_u64)
        .map(|value| value.clamp(1_000, 100_000));

    Ok(ExaSearchArgs {
        query,
        search_type,
        num_results,
        livecrawl,
        context_max_characters,
    })
}

fn enum_string(
    input: &Value,
    field: &str,
    allowed: &[&str],
    default: &str,
) -> Result<String, ToolError> {
    let value = input
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default);
    if allowed.contains(&value) {
        Ok(value.to_string())
    } else {
        Err(ToolError::InvalidInput(format!(
            "{field} must be one of {}",
            allowed.join(", ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{DEFAULT_LIVECRAWL, DEFAULT_NUM_RESULTS, DEFAULT_SEARCH_TYPE, parse_search_args};

    #[test]
    fn search_args_apply_opencode_compatible_defaults() {
        let args = parse_search_args(&json!({ "query": "rust async search" })).expect("args");

        assert_eq!(args.query, "rust async search");
        assert_eq!(args.num_results, DEFAULT_NUM_RESULTS);
        assert_eq!(args.livecrawl, DEFAULT_LIVECRAWL);
        assert_eq!(args.search_type, DEFAULT_SEARCH_TYPE);
        assert_eq!(args.context_max_characters, None);
    }

    #[test]
    fn search_args_validate_enums() {
        let err = parse_search_args(&json!({
            "query": "rust",
            "type": "slow"
        }))
        .expect_err("invalid enum");

        assert!(err.to_string().contains("type must be one of"));
    }

    #[test]
    fn search_args_accept_camel_case_context_limit() {
        let args = parse_search_args(&json!({
            "query": "rust",
            "numResults": 100,
            "contextMaxCharacters": 500000
        }))
        .expect("args");

        assert_eq!(args.num_results, 20);
        assert_eq!(args.context_max_characters, Some(100_000));
    }
}
