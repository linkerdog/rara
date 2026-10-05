use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use crate::tui::message_role::MessageRole;
use crate::tui::state::TuiApp;

pub(super) const INIT_PROMPT: &str = "Inspect this repository and create or update AGENTS.md with concise, evidence-based instructions for future coding agents. Read existing AGENTS.md files and preserve applicable guidance. Describe the architecture, build and test commands, and important local conventions that you can verify from this repository. Avoid generic advice and invented commands. Follow the current permissions and planning mode; use the normal editing and approval flow.";

pub(super) fn copy(app: &mut TuiApp, arg: Option<&str>) {
    if arg.is_some_and(|arg| arg != "code") {
        app.push_notice("Usage: /copy [code]");
        return;
    }
    let answer = app
        .committed_turns
        .iter()
        .chain((!app.is_busy()).then_some(&*app.active_turn))
        .rev()
        .flat_map(|turn| turn.entries.iter().rev())
        .find(|entry| entry.role == MessageRole::Agent && !entry.message.trim().is_empty())
        .map(|entry| entry.message.clone());
    let Some(answer) = answer else {
        app.push_notice("No completed assistant response to copy.");
        return;
    };
    let text = if arg == Some("code") {
        let mut last = None;
        let mut current = None;
        for event in Parser::new(&answer) {
            match event {
                Event::Start(Tag::CodeBlock(_)) => current = Some(String::new()),
                Event::Text(text) => {
                    if let Some(code) = &mut current {
                        code.push_str(&text);
                    }
                }
                Event::End(TagEnd::CodeBlock) => last = current.take(),
                _ => {}
            }
        }
        let Some(code) = last else {
            app.push_notice("The last assistant response has no code block.");
            return;
        };
        code
    } else {
        answer
    };
    let notice = app
        .clipboard
        .get_or_insert_with(crate::tui::clipboard::Clipboard::from_environment)
        .request(text);
    app.push_notice(notice);
}
