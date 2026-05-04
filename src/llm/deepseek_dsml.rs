use std::borrow::Cow;

use nom::bytes::complete::{tag, take_until};
use nom::character::complete::multispace0;
use nom::error::{Error, ErrorKind};
use nom::{Err as NomErr, IResult, Parser};
use serde_json::Value;

const DSML_TOKENS: [&str; 2] = ["｜DSML｜", "|DSML|"];
const TOOL_CALLS_BLOCK_NAME: &str = "tool_calls";
const INVOKE_TAG_NAME: &str = "invoke";
const PARAMETER_TAG_NAME: &str = "parameter";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DeepSeekDsmlToolCall {
    pub(crate) name: String,
    pub(crate) input: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DeepSeekDsmlExtraction {
    pub(crate) visible_text: String,
    pub(crate) tool_calls: Vec<DeepSeekDsmlToolCall>,
}

pub(crate) fn contains_dsml(text: &str) -> bool {
    DSML_TOKENS.iter().any(|token| text.contains(token))
}

pub(crate) fn extract_tool_calls_from_text(text: &str) -> DeepSeekDsmlExtraction {
    let mut visible_text = String::new();
    let mut tool_calls = Vec::new();
    let mut rest = text;

    while let Some(open) = find_next_open_tag(rest, TOOL_CALLS_BLOCK_NAME) {
        visible_text.push_str(&rest[..open.start]);
        let after_open = &rest[open.start + open.tag.len()..];
        let close_tag = close_tag(open.token, TOOL_CALLS_BLOCK_NAME);
        let Some(close_start) = after_open.find(&close_tag) else {
            visible_text.push_str(&rest[open.start..]);
            rest = "";
            break;
        };

        let block_body = &after_open[..close_start];
        let candidate =
            &rest[open.start..open.start + open.tag.len() + close_start + close_tag.len()];
        match parse_tool_call_block(block_body, open.token) {
            Some(mut calls) => tool_calls.append(&mut calls),
            None => visible_text.push_str(candidate),
        }
        rest = &after_open[close_start + close_tag.len()..];
    }

    visible_text.push_str(rest);
    DeepSeekDsmlExtraction {
        visible_text,
        tool_calls,
    }
}

pub(crate) fn strip_tool_call_blocks(text: &str) -> Cow<'_, str> {
    let extraction = extract_tool_calls_from_text(text);
    if extraction.tool_calls.is_empty() || extraction.visible_text == text {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(extraction.visible_text)
    }
}

pub(crate) fn contains_orphaned_tool_call_markup(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && DSML_TOKENS.into_iter().any(|token| {
            [
                close_tag(token, TOOL_CALLS_BLOCK_NAME),
                close_tag(token, INVOKE_TAG_NAME),
                close_tag(token, PARAMETER_TAG_NAME),
            ]
            .into_iter()
            .any(|tag| text.contains(tag.as_str()))
        })
}

type NomResult<'a, T> = IResult<&'a str, T>;

fn parse_tool_call_block(block: &str, token: &'static str) -> Option<Vec<DeepSeekDsmlToolCall>> {
    let (rest, calls) = parse_tool_call_block_nom(block, token).ok()?;
    if !rest.trim().is_empty() || calls.is_empty() {
        return None;
    }
    Some(calls)
}

fn parse_tool_call_block_nom<'a>(
    input: &'a str,
    token: &'static str,
) -> NomResult<'a, Vec<DeepSeekDsmlToolCall>> {
    let mut calls = Vec::new();
    let (mut rest, _) = multispace0.parse(input)?;
    while !rest.is_empty() {
        let (next, call) = parse_invoke_nom(rest, token)?;
        calls.push(call);
        let (next, _) = multispace0.parse(next)?;
        rest = next;
    }
    Ok((rest, calls))
}

fn parse_invoke_nom<'a>(
    input: &'a str,
    token: &'static str,
) -> NomResult<'a, DeepSeekDsmlToolCall> {
    let input = input.trim_start();
    let open_tag = open_tag(token, INVOKE_TAG_NAME);
    let (input, _) = tag(open_tag.as_str()).parse(input)?;
    let (input, attrs) = take_until(">").parse(input)?;
    let (input, _) = tag(">").parse(input)?;
    let name = quoted_attr(attrs, "name").ok_or_else(|| nom_error(input, ErrorKind::Tag))?;
    if name.is_empty() {
        return Err(nom_error(input, ErrorKind::Tag));
    }

    let close_tag = close_tag(token, INVOKE_TAG_NAME);
    let (input, body) = take_until(close_tag.as_str()).parse(input)?;
    let (input, _) = tag(close_tag.as_str()).parse(input)?;
    let (_, parameters) = parse_parameters_nom(body, token)?;
    Ok((
        input,
        DeepSeekDsmlToolCall {
            name: name.to_string(),
            input: parameters,
        },
    ))
}

fn parse_parameters_nom<'a>(body: &'a str, token: &'static str) -> NomResult<'a, Value> {
    let mut params = serde_json::Map::new();
    let (mut rest, _) = multispace0.parse(body)?;

    while !rest.is_empty() {
        let (next, (name, value)) = parse_parameter_nom(rest, token)?;
        params.insert(name, value);
        let (next, _) = multispace0.parse(next)?;
        rest = next;
    }

    Ok((rest, Value::Object(params)))
}

fn parse_parameter_nom<'a>(input: &'a str, token: &'static str) -> NomResult<'a, (String, Value)> {
    let open_tag = open_tag(token, PARAMETER_TAG_NAME);
    let close_tag = close_tag(token, PARAMETER_TAG_NAME);

    let (input, _) = tag(open_tag.as_str()).parse(input)?;
    let (input, attrs) = take_until(">").parse(input)?;
    let (input, _) = tag(">").parse(input)?;

    let name = quoted_attr(attrs, "name").ok_or_else(|| nom_error(input, ErrorKind::Tag))?;
    if name.is_empty() {
        return Err(nom_error(input, ErrorKind::Tag));
    }
    let is_string = match quoted_attr(attrs, "string") {
        Some("true") => true,
        Some("false") | None => false,
        Some(_) => return Err(nom_error(input, ErrorKind::Tag)),
    };

    let (input, raw_value) = take_until(close_tag.as_str()).parse(input)?;
    let (input, _) = tag(close_tag.as_str()).parse(input)?;
    let value = if is_string {
        Value::String(raw_value.to_string())
    } else {
        serde_json::from_str(raw_value.trim())
            .unwrap_or_else(|_| Value::String(raw_value.trim().to_string()))
    };
    Ok((input, (name.to_string(), value)))
}

#[derive(Debug)]
struct OpenTag {
    start: usize,
    token: &'static str,
    tag: String,
}

fn find_next_open_tag(input: &str, name: &str) -> Option<OpenTag> {
    DSML_TOKENS
        .into_iter()
        .filter_map(|token| {
            let tag = exact_open_tag(token, name);
            input.find(&tag).map(|start| OpenTag { start, token, tag })
        })
        .min_by_key(|found| found.start)
}

fn exact_open_tag(token: &str, name: &str) -> String {
    format!("<{token}{name}>")
}

fn open_tag(token: &str, name: &str) -> String {
    format!("<{token}{name}")
}

fn close_tag(token: &str, name: &str) -> String {
    format!("</{token}{name}>")
}

fn quoted_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn nom_error(input: &str, kind: ErrorKind) -> NomErr<Error<&str>> {
    NomErr::Error(Error::new(input, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fullwidth_dsml_tool_call_and_preserves_visible_text() {
        let extraction = extract_tool_calls_from_text(
            "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\n<｜DSML｜parameter name=\"path\" string=\"true\">src/lib.rs</｜DSML｜parameter>\n<｜DSML｜parameter name=\"options\" string=\"false\">{\"limit\":20}</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>\nAfter",
        );

        assert_eq!(extraction.visible_text, "Before\n\nAfter");
        assert_eq!(extraction.tool_calls.len(), 1);
        assert_eq!(extraction.tool_calls[0].name, "read_file");
        assert_eq!(extraction.tool_calls[0].input["path"], "src/lib.rs");
        assert_eq!(extraction.tool_calls[0].input["options"]["limit"], 20);
    }

    #[test]
    fn extracts_ascii_pipe_dsml_tool_call_as_pdf_compatibility_fallback() {
        let extraction = extract_tool_calls_from_text(
            "Before\n<|DSML|tool_calls>\n<|DSML|invoke name=\"list_files\">\n<|DSML|parameter name=\"path\" string=\"true\">src</|DSML|parameter>\n</|DSML|invoke>\n</|DSML|tool_calls>\nAfter",
        );

        assert_eq!(extraction.visible_text, "Before\n\nAfter");
        assert_eq!(extraction.tool_calls.len(), 1);
        assert_eq!(extraction.tool_calls[0].name, "list_files");
        assert_eq!(extraction.tool_calls[0].input["path"], "src");
    }

    #[test]
    fn preserves_malformed_dsml_without_closing_block() {
        let input = "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\nAfter";
        let extraction = extract_tool_calls_from_text(input);

        assert_eq!(extraction.visible_text, input);
        assert!(extraction.tool_calls.is_empty());
    }

    #[test]
    fn detects_orphaned_dsml_closing_markup() {
        assert!(contains_orphaned_tool_call_markup(
            "kind: value\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>"
        ));
        assert!(contains_orphaned_tool_call_markup("path</|DSML|parameter>"));
    }

    #[test]
    fn ignores_plain_text_and_open_malformed_blocks_as_orphaned_markup() {
        assert!(!contains_orphaned_tool_call_markup("The status is: ok"));
        assert!(!contains_orphaned_tool_call_markup(
            "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\nAfter"
        ));
    }
}
