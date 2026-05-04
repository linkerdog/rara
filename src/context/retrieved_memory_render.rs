pub(crate) const MEMORY_CONTEXT_HEADER: &str = "<rara_internal_history_context>";
pub(crate) const MEMORY_CONTEXT_FOOTER: &str = "</rara_internal_history_context>";

#[derive(Debug, Clone, Copy)]
pub(crate) struct RetrievedMemoryRenderItem<'a> {
    pub label: &'a str,
    pub detail: &'a str,
}

pub(crate) fn render_retrieved_memory_context(
    items: &[RetrievedMemoryRenderItem<'_>],
) -> Option<String> {
    if items.is_empty() {
        return None;
    }

    let mut lines = vec![
        MEMORY_CONTEXT_HEADER.to_string(),
        "Relevant memory selected for the current turn:".to_string(),
    ];
    for item in items {
        lines.push(render_retrieved_memory_context_item(*item));
    }
    lines.extend([
        "".to_string(),
        "Use this as background recall. If it conflicts with the current user request or inspected files, trust the current evidence."
            .to_string(),
        MEMORY_CONTEXT_FOOTER.to_string(),
    ]);

    Some(lines.join("\n"))
}

pub(crate) fn render_retrieved_memory_context_item(item: RetrievedMemoryRenderItem<'_>) -> String {
    format!(
        "- {}: {}",
        normalize_memory_context_field(item.label),
        normalize_memory_context_field(item.detail)
    )
}

fn normalize_memory_context_field(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_retrieved_memory_context_normalizes_multiline_items() {
        let rendered = render_retrieved_memory_context(&[RetrievedMemoryRenderItem {
            label: "Memory:\nReference",
            detail: "first line\nsecond\tline",
        }])
        .expect("retrieved memory context should render");

        assert!(rendered.contains("- Memory: Reference: first line second line"));
        assert!(!rendered.contains("first line\nsecond"));
    }
}
