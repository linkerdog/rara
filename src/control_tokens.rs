use std::borrow::Cow;

use crate::llm::deepseek_dsml;

const INTERNAL_BLOCK_TAGS: [&str; 3] = [
    "agent_runtime",
    "agent_runtime_error",
    "rara_internal_history_context",
];
pub(crate) const DEEPSEEK_EOS: &str = "<｜end▁of▁sentence｜>";

pub(crate) fn scrub_internal_control_tokens(message: &str) -> String {
    let had_deepseek_dsml = deepseek_dsml::contains_dsml(message);
    let had_deepseek_eos = message.contains(DEEPSEEK_EOS);

    let message = if had_deepseek_dsml {
        strip_deepseek_v4_dsml_control_blocks(message)
    } else {
        Cow::Borrowed(message)
    };
    let message =
        if (had_deepseek_dsml || had_deepseek_eos) && message.trim_start().starts_with("<think>") {
            strip_deepseek_leading_think_block(&message)
        } else {
            message
        };
    let message = if had_deepseek_eos {
        Cow::Owned(message.replace(DEEPSEEK_EOS, ""))
    } else {
        message
    };
    let message = strip_internal_blocks(&message);
    if !message.contains('<') {
        return message.into_owned();
    }

    strip_legacy_control_markers(&message)
}

pub(crate) fn has_deepseek_control_evidence(message: &str) -> bool {
    deepseek_dsml::contains_dsml(message) || message.contains(DEEPSEEK_EOS)
}

pub(crate) fn scrub_deepseek_visible_text(message: &str, has_control_evidence: bool) -> String {
    let message = if has_control_evidence && message.trim_start().starts_with("<think>") {
        strip_deepseek_leading_think_block(message)
    } else {
        Cow::Borrowed(message)
    };
    message.replace(DEEPSEEK_EOS, "")
}

pub(crate) fn has_pending_internal_control_context(message: &str) -> bool {
    has_open_internal_block(message)
        || has_open_leading_think_block(message)
        || has_open_deepseek_dsml_tool_block(message)
        || ends_with_possible_control_prefix(message)
}

fn strip_legacy_control_markers(message: &str) -> String {
    let mut cleaned = String::with_capacity(message.len());
    let mut last_copied = 0usize;
    let mut idx = 0usize;
    while idx < message.len() {
        let Some(ch) = message[idx..].chars().next() else {
            break;
        };
        if ch != '<' {
            idx += ch.len_utf8();
            continue;
        }

        let Some(end_idx) = legacy_control_marker_end(message, idx) else {
            idx += ch.len_utf8();
            continue;
        };

        cleaned.push_str(&message[last_copied..idx]);
        if cleaned.chars().last().is_some_and(|c| !c.is_whitespace()) {
            cleaned.push('\n');
        }
        idx = end_idx + 2;
        last_copied = idx;
    }
    cleaned.push_str(&message[last_copied..]);
    cleaned
}

fn legacy_control_marker_end(message: &str, start: usize) -> Option<usize> {
    let mut idx = start + '<'.len_utf8();
    let mut saw_candidate = false;
    while idx < message.len() {
        let ch = message[idx..].chars().next()?;
        if ch == '|' && message[idx + ch.len_utf8()..].starts_with('>') {
            return saw_candidate.then_some(idx);
        }
        if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
            return None;
        }
        saw_candidate = true;
        idx += ch.len_utf8();
    }
    None
}

fn strip_internal_blocks(message: &str) -> Cow<'_, str> {
    let mut output = Cow::Borrowed(message);
    for tag in INTERNAL_BLOCK_TAGS {
        output = strip_balanced_or_open_internal_block(output, tag);
    }
    output
}

fn strip_balanced_or_open_internal_block<'a>(message: Cow<'a, str>, tag: &str) -> Cow<'a, str> {
    let open = format!("<{tag}>");
    if !message.contains(open.as_str()) {
        return message;
    }

    let close = format!("</{tag}>");
    let mut remaining = message.as_ref();
    let mut cleaned = String::with_capacity(remaining.len());
    while let Some(start) = remaining.find(open.as_str()) {
        cleaned.push_str(&remaining[..start]);
        let after_open = &remaining[start + open.len()..];
        let Some(end) = after_open.find(close.as_str()) else {
            remaining = "";
            break;
        };
        remaining = &after_open[end + close.len()..];
    }
    cleaned.push_str(remaining);
    Cow::Owned(cleaned)
}

fn has_open_internal_block(message: &str) -> bool {
    INTERNAL_BLOCK_TAGS.iter().any(|tag| {
        let open = format!("<{tag}>");
        let Some(open_idx) = message.rfind(open.as_str()) else {
            return false;
        };
        let close = format!("</{tag}>");
        !message[open_idx + open.len()..].contains(close.as_str())
    })
}

fn has_open_leading_think_block(message: &str) -> bool {
    let trimmed = message.trim_start();
    trimmed.starts_with("<think>") && !trimmed.contains("</think>")
}

fn has_open_deepseek_dsml_tool_block(message: &str) -> bool {
    ["<｜DSML｜tool_calls>", "<|DSML|tool_calls>"]
        .into_iter()
        .any(|open| {
            let Some(open_idx) = message.rfind(open) else {
                return false;
            };
            let close = if open.contains("｜DSML｜") {
                "</｜DSML｜tool_calls>"
            } else {
                "</|DSML|tool_calls>"
            };
            !message[open_idx + open.len()..].contains(close)
        })
}

fn ends_with_possible_control_prefix(message: &str) -> bool {
    let suffix = message
        .rsplit_once(|c: char| c.is_whitespace())
        .map(|(_, tail)| tail)
        .unwrap_or(message);
    if suffix.is_empty() {
        return false;
    }
    [
        "<think>",
        "<agent_runtime>",
        "<agent_runtime_error>",
        "<rara_internal_history_context>",
        "<｜DSML｜tool_calls>",
        "<|DSML|tool_calls>",
        "<｜end▁of▁sentence｜>",
    ]
    .into_iter()
    .any(|token| token.starts_with(suffix))
        || is_possible_legacy_control_marker_prefix(suffix)
}

fn is_possible_legacy_control_marker_prefix(suffix: &str) -> bool {
    let Some(candidate) = suffix.strip_prefix('<') else {
        return false;
    };
    !candidate.contains("|>")
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn strip_deepseek_leading_think_block(message: &str) -> Cow<'_, str> {
    const THINK_OPEN: &str = "<think>";
    const THINK_CLOSE: &str = "</think>";

    let trimmed = message.trim_start();
    if !trimmed.starts_with(THINK_OPEN) {
        return Cow::Borrowed(message);
    }

    let block = &trimmed[THINK_OPEN.len()..];
    match block.find(THINK_CLOSE) {
        Some(close_idx) => Cow::Owned(block[close_idx + THINK_CLOSE.len()..].to_string()),
        None => Cow::Owned(String::new()),
    }
}

fn strip_deepseek_v4_dsml_control_blocks(message: &str) -> Cow<'_, str> {
    if !deepseek_dsml::contains_dsml(message) {
        return Cow::Borrowed(message);
    }

    let output = deepseek_dsml::strip_tool_call_blocks(message);
    if looks_like_orphaned_deepseek_v4_dsml_payload(output.trim()) {
        Cow::Owned(String::new())
    } else {
        output
    }
}

fn looks_like_orphaned_deepseek_v4_dsml_payload(text: &str) -> bool {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return false;
    }

    let code_like = lines
        .iter()
        .filter(|line| {
            line.starts_with('}')
                || line.ends_with('{')
                || line.ends_with("},")
                || line.contains(": ")
                || line.starts_with("let ")
                || line.starts_with("MemorySelectionCandidate")
        })
        .count();
    code_like * 2 >= lines.len()
}
