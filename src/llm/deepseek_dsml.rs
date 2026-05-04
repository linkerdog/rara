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

pub(crate) fn strip_orphaned_tool_call_tail(text: &str) -> Cow<'_, str> {
    let Some(tail_start) = orphaned_tool_call_tail_start(text) else {
        return Cow::Borrowed(text);
    };

    let prefix = text[..tail_start].trim_end();
    if prefix.is_empty() || looks_like_leaked_tool_argument_prefix(prefix) {
        Cow::Owned(String::new())
    } else {
        Cow::Owned(prefix.to_string())
    }
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

fn orphaned_tool_call_tail_start(text: &str) -> Option<usize> {
    let mut line_start = 0usize;
    while line_start < text.len() {
        let line_end = text[line_start..]
            .find('\n')
            .map(|offset| line_start + offset)
            .unwrap_or(text.len());
        let line = &text[line_start..line_end];
        if starts_with_dsml_tag(line.trim_start()) {
            let tail = &text[line_start..];
            if has_orphaned_tool_call_closing_sequence(tail) {
                return Some(line_start);
            }
        }

        if line_end == text.len() {
            break;
        }
        line_start = line_end + '\n'.len_utf8();
    }

    None
}

fn starts_with_dsml_tag(line: &str) -> bool {
    DSML_TOKENS.iter().any(|token| {
        [
            exact_open_tag(token, TOOL_CALLS_BLOCK_NAME),
            open_tag(token, INVOKE_TAG_NAME),
            open_tag(token, PARAMETER_TAG_NAME),
            close_tag(token, TOOL_CALLS_BLOCK_NAME),
            close_tag(token, INVOKE_TAG_NAME),
            close_tag(token, PARAMETER_TAG_NAME),
        ]
        .into_iter()
        .any(|tag| line.starts_with(tag.as_str()))
    })
}

fn has_orphaned_tool_call_closing_sequence(text: &str) -> bool {
    DSML_TOKENS.iter().any(|token| {
        text.contains(close_tag(token, TOOL_CALLS_BLOCK_NAME).as_str())
            || text.contains(close_tag(token, INVOKE_TAG_NAME).as_str())
    })
}

fn looks_like_leaked_tool_argument_prefix(prefix: &str) -> bool {
    let mut saw_line = false;
    for line in prefix
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        saw_line = true;
        if !looks_like_leaked_tool_argument_line(line) {
            return false;
        }
    }
    saw_line
}

fn looks_like_leaked_tool_argument_line(line: &str) -> bool {
    matches!(line, "}" | "},")
        || line.ends_with('{')
        || line.ends_with("},")
        || looks_like_struct_field_line(line)
}

fn looks_like_struct_field_line(line: &str) -> bool {
    let Some((field, value)) = line.split_once(':') else {
        return false;
    };
    let field = field.trim();
    let value = value.trim();
    !field.is_empty()
        && !value.is_empty()
        && field
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        && (line.ends_with(',') || value.contains("format!("))
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
    fn strips_orphaned_dsml_tail_and_preserves_visible_prefix() {
        let cleaned = strip_orphaned_tool_call_tail(
            "Visible answer.\n<｜DSML｜parameter name=\"path\" string=\"true\">src/lib.rs</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>",
        );

        assert_eq!(cleaned.as_ref(), "Visible answer.");
    }

    #[test]
    fn drops_orphaned_dsml_tail_with_leaked_argument_prefix() {
        let cleaned = strip_orphaned_tool_call_tail(
            "kind: format!(\"unknown_{tool_name}\"),\nlabel: format!(\"Unknown ({tool_name})\"),\n}\n<｜DSML｜parameter name=\"path\" string=\"true\">src/lib.rs</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>",
        );

        assert!(cleaned.trim().is_empty());
    }

    #[test]
    fn preserves_literal_dsml_closing_markup() {
        let inline = "Document `path</|DSML|parameter>` literally.";
        assert_eq!(strip_orphaned_tool_call_tail(inline).as_ref(), inline);

        let multiline = "Document this literal:\n</|DSML|parameter>";
        assert_eq!(strip_orphaned_tool_call_tail(multiline).as_ref(), multiline);
    }

    #[test]
    fn ignores_plain_text_and_open_malformed_blocks_as_orphaned_tail() {
        assert_eq!(
            strip_orphaned_tool_call_tail("The status is: ok").as_ref(),
            "The status is: ok"
        );
        let malformed = "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\nAfter";
        assert_eq!(
            strip_orphaned_tool_call_tail(malformed).as_ref(),
            "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\nAfter"
        );
    }
}
