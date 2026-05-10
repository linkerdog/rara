pub(crate) fn compact_delegate_rest(rest: &str) -> Option<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(rest) {
        if let Some(name) = value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let instruction = value
                .get("instruction")
                .and_then(serde_json::Value::as_str)
                .map(compact_instruction)
                .unwrap_or_else(|| "instruction unavailable".to_string());
            return Some(format!("{name}: {instruction}"));
        }
        return value
            .get("instruction")
            .and_then(serde_json::Value::as_str)
            .map(compact_instruction);
    }
    Some(compact_instruction(rest))
}

pub(crate) fn compact_instruction(instruction: &str) -> String {
    const MAX_CHARS: usize = 120;
    let normalized = instruction.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX_CHARS {
        return normalized;
    }
    let mut truncated = normalized.chars().take(MAX_CHARS).collect::<String>();
    truncated.push('…');
    truncated
}

pub(crate) fn bash_rg_exploration_action_label(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() || !contains_rg_invocation(command) {
        return None;
    }

    let tokens = command.split_whitespace().collect::<Vec<_>>();
    let rg_index = tokens.iter().position(|part| *part == "rg")?;
    let cwd = command_prefix_cwd(tokens.as_slice(), rg_index);
    let args = &tokens[rg_index + 1..];

    if args.contains(&"--files") {
        let target = args
            .iter()
            .rfind(|part| !part.starts_with('-'))
            .copied()
            .unwrap_or("workspace");
        Some(format!("Find files {}", display_path_in_cwd(cwd, target)))
    } else {
        let terms = args
            .iter()
            .filter(|part| !part.starts_with('-'))
            .map(|part| part.trim_matches('"').trim_matches('\''))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        match terms.as_slice() {
            [] => Some("Search workspace".to_string()),
            [query] => Some(format!("Search {query}")),
            [query, target, ..] => Some(format!(
                "Search {query} {}",
                display_path_in_cwd(cwd, target)
            )),
        }
    }
}

fn contains_rg_invocation(command: &str) -> bool {
    command
        .split([';', '|', '&'])
        .map(str::trim)
        .any(|segment| segment == "rg" || segment.starts_with("rg ") || segment.starts_with("rg\t"))
}

fn command_prefix_cwd<'a>(tokens: &'a [&str], rg_index: usize) -> Option<&'a str> {
    if rg_index >= 3 && tokens.first() == Some(&"cd") && tokens.get(2) == Some(&"&&") {
        tokens.get(1).copied()
    } else {
        None
    }
}

fn display_path_in_cwd(cwd: Option<&str>, target: &str) -> String {
    let target = target.trim_matches('"').trim_matches('\'');
    match cwd {
        Some(cwd)
            if target != "workspace"
                && !target.starts_with('/')
                && target != "."
                && !target.starts_with("./") =>
        {
            format!("{}/{}", cwd.trim_end_matches('/'), target)
        }
        Some(cwd) if target == "." => cwd.to_string(),
        _ => target.to_string(),
    }
}
