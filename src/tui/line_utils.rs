use ratatui::text::{Line, Span};

pub fn line_to_static(line: &Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .iter()
            .map(|span| Span {
                style: span.style,
                content: std::borrow::Cow::Owned(span.content.to_string()),
            })
            .collect(),
    }
}

pub fn push_owned_lines(src: &[Line<'_>], out: &mut Vec<Line<'static>>) {
    for line in src {
        out.push(line_to_static(line));
    }
}

pub fn prefix_lines(
    lines: Vec<Line<'static>>,
    initial_prefix: Span<'static>,
    subsequent_prefix: Span<'static>,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(if idx == 0 {
                initial_prefix.clone()
            } else {
                subsequent_prefix.clone()
            });
            spans.extend(line.spans);
            Line::from(spans).style(line.style)
        })
        .collect()
}
