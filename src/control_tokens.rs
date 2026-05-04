use std::borrow::Cow;

use crate::llm::deepseek_dsml;

const INTERNAL_BLOCK_TAGS: [&str; 3] = [
    "agent_runtime",
    "agent_runtime_error",
    "rara_internal_history_context",
];

pub(crate) fn scrub_internal_control_tokens(message: &str) -> String {
    let had_deepseek_dsml = deepseek_dsml::contains_dsml(message);
    let had_deepseek_eos = message.contains("<｜end▁of▁sentence｜>");

    let message = if had_deepseek_dsml {
        strip_deepseek_v4_dsml_control_blocks(message)
    } else {
        Cow::Borrowed(message)
    };
    let message = if message.trim_start().starts_with("<think>") {
        strip_deepseek_leading_think_block(&message)
    } else {
        message
    };
    let message = if had_deepseek_eos {
        Cow::Owned(message.replace("<｜end▁of▁sentence｜>", ""))
    } else {
        message
    };
    let message = strip_internal_blocks(&message);
    if !message.contains('<') {
        return message.into_owned();
    }

    let mut cleaned = String::with_capacity(message.len());
    let mut chars = message.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if ch == '<' {
            if let Some(end) = message[idx..].find("|>") {
                let end_idx = idx + end;
                let candidate = &message[idx + 1..end_idx];
                if !candidate.is_empty()
                    && candidate
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    if cleaned.chars().last().is_some_and(|c| !c.is_whitespace()) {
                        cleaned.push('\n');
                    }
                    let skip_to = end_idx + 2;
                    while let Some(&(next_idx, _)) = chars.peek() {
                        if next_idx < skip_to {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    continue;
                }
            }
        }
        cleaned.push(ch);
    }

    cleaned
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
