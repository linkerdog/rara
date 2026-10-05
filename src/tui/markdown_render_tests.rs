use insta::assert_snapshot;
use ratatui::style::{Modifier, Style};

use super::*;

/// Helper that includes style information (modifiers + foreground color) so
/// snapshots capture visual rendering intent, not just text structure.
fn render_to_string(md: &str) -> String {
    render_markdown_text(md)
        .lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|span| {
                    let content = &span.content;
                    let style = span.style;
                    let tags = style_modifier_tags(style);
                    if tags.is_empty() {
                        content.to_string()
                    } else {
                        format!("{}({})", tags, content)
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn render_to_string_width(md: &str, width: usize) -> String {
    render_markdown_text_with_width(md, Some(width))
        .lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|span| {
                    let content = &span.content;
                    let style = span.style;
                    let tags = style_modifier_tags(style);
                    if tags.is_empty() {
                        content.to_string()
                    } else {
                        format!("{}({})", tags, content)
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn style_modifier_tags(style: Style) -> String {
    let mut tags = String::new();
    if style.add_modifier.contains(Modifier::BOLD) {
        tags.push_str("B+");
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        tags.push_str("I+");
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        tags.push_str("U+");
    }
    if style.add_modifier.contains(Modifier::DIM) {
        tags.push_str("dim+");
    }
    if style.add_modifier.contains(Modifier::CROSSED_OUT) {
        tags.push_str("S+");
    }
    // Strip trailing '+'
    tags.trim_end_matches('+').to_string()
}

#[test]
fn markdown_headings() {
    let md = "# H1\n## H2\n### H3\n\n#### Not bold (h4+)";
    assert_snapshot!("markdown_headings", render_to_string(md));
}

#[test]
fn markdown_bold_italic() {
    let md = "**bold** and *italic* and ***both***";
    assert_snapshot!("markdown_bold_italic", render_to_string(md));
}

#[test]
fn markdown_inline_code() {
    let md = "Use `unwrap_or_default()` for safety.";
    assert_snapshot!("markdown_inline_code", render_to_string(md));
}

#[test]
fn markdown_code_block_with_lang() {
    let md = "```rust\nfn main() {\n    println!(\"hi\");\n}\n```";
    assert_snapshot!("markdown_code_block_with_lang", render_to_string(md));
}

#[test]
fn markdown_code_block_no_lang() {
    let md = "```\necho hello world\n```";
    assert_snapshot!("markdown_code_block_no_lang", render_to_string(md));
}

#[test]
fn markdown_ordered_list() {
    let md = "1. First\n2. Second\n3. Third\n";
    assert_snapshot!("markdown_ordered_list", render_to_string(md));
}

#[test]
fn markdown_unordered_list() {
    let md = "- item one\n- item two\n- item three\n";
    assert_snapshot!("markdown_unordered_list", render_to_string(md));
}

#[test]
fn markdown_task_list() {
    let md = "- [ ] todo\n- [x] done\n- [ ] another todo\n";
    assert_snapshot!("markdown_task_list", render_to_string(md));
}

#[test]
fn markdown_blockquote() {
    let md = "> This is a blockquote.\n> It spans multiple lines.\n";
    assert_snapshot!("markdown_blockquote", render_to_string(md));
}

#[test]
fn markdown_nested_blockquote() {
    let md = "> level one\n>> level two\n> back to one\n";
    assert_snapshot!("markdown_nested_blockquote", render_to_string(md));
}

#[test]
fn markdown_table() {
    let md = concat!(
        "| Name  | Value | Notes     |\n",
        "|-------|-------|-----------|\n",
        "| alpha | 1     | first     |\n",
        "| beta  | 22    | second    |\n"
    );
    assert_snapshot!("markdown_table", render_to_string(md));
}

#[test]
fn markdown_table_narrow_wrapping() {
    let md = concat!(
        "| Column A | Column B | Column C |\n",
        "|----------|----------|----------|\n",
        "| long long long value | short | also quite long here |\n",
    );
    assert_snapshot!(
        "markdown_table_narrow_wrapping",
        render_to_string_width(md, 40)
    );
}

#[test]
fn markdown_links() {
    let md = "See [the docs](https://example.com) and also [Copilot](https://copilot.github.com).";
    assert_snapshot!("markdown_links", render_to_string(md));
}

#[test]
fn markdown_thematic_break() {
    let md = "above\n\n---\n\nbelow";
    assert_snapshot!("markdown_thematic_break", render_to_string(md));
}

#[test]
fn markdown_comprehensive() {
    let md = concat!(
        "# Overview\n\n",
        "This is a **bold** statement with *emphasis*.\n\n",
        "## Steps\n\n",
        "1. Install with `cargo install rara`\n",
        "2. Run `rara init`\n\n",
        "```rust\n",
        "// example code\n",
        "fn main() {\n",
        "    println!(\"ready\");\n",
        "}\n",
        "```\n\n",
        "> Note: this is important.\n\n",
        "- [x] done task\n",
        "- [ ] pending\n",
    );
    assert_snapshot!("markdown_comprehensive", render_to_string(md));
}

#[test]
fn markdown_nested_list() {
    let md = "- top\n  - nested 1\n    - nested 2\n  - back to 1\n- top again\n";
    assert_snapshot!("markdown_nested_list", render_to_string(md));
}

#[test]
fn markdown_gfm_extensions() {
    let md = concat!(
        "~~struck~~ normal.\n\n",
        "Auto-link: https://example.com/page\n\n",
        "Footnote ref[^1].\n\n",
        "[^1]: This is the footnote.\n",
    );
    assert_snapshot!("markdown_gfm_extensions", render_to_string(md));
}
