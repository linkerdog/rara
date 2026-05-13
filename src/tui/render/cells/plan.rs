pub(super) fn split_progress_sentences(message: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let mut chars = message.chars().peekable();

    while let Some(ch) = chars.next() {
        current.push(ch);

        let next = chars.peek().copied();
        let previous = current.chars().rev().nth(1);
        let is_decimal_separator = ch == '.'
            && previous.is_some_and(|prev| prev.is_ascii_digit())
            && next.is_some_and(|peek| peek.is_ascii_digit());
        let continues_punctuation = next.is_some_and(|peek| matches!(peek, '.' | '!' | '?'));

        if matches!(ch, '.' | '!' | '?') && !is_decimal_separator && !continues_punctuation {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                sentences.push(trimmed.to_string());
            }
            current.clear();
        }
    }

    let tail = current.trim();
    if !tail.is_empty() {
        sentences.push(tail.to_string());
    }

    sentences
}

pub(super) fn is_structured_response_marker(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("<proposed_plan>")
        || trimmed.starts_with("</proposed_plan>")
        || trimmed.starts_with("<plan>")
        || trimmed.starts_with("</plan>")
        || trimmed.starts_with("<request_user_input>")
        || trimmed.starts_with("</request_user_input>")
        || trimmed.starts_with("<continue_inspection")
}

pub(super) fn is_structured_progress_list_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- [")
        || trimmed.starts_with("* [")
        || trimmed.starts_with("• [")
        || trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("• ")
}

pub(super) fn compact_live_response_source(message: &str) -> Option<String> {
    let mut retained = Vec::new();
    let mut saw_prose = false;
    let mut in_structured_block = false;

    for line in message.lines() {
        let trimmed = line.trim();
        if is_structured_response_marker(trimmed) {
            if trimmed.starts_with("</") {
                in_structured_block = false;
            } else if trimmed.starts_with('<') && trimmed.ends_with('>') && !trimmed.ends_with("/>")
            {
                in_structured_block = true;
            }
            continue;
        }

        if in_structured_block {
            continue;
        }

        if trimmed.is_empty() {
            if saw_prose {
                retained.push(String::new());
            }
            continue;
        }

        if is_structured_progress_list_line(trimmed) && saw_prose {
            continue;
        }

        retained.push(trimmed.to_string());
        saw_prose = true;
    }

    let compact = retained
        .into_iter()
        .skip_while(|line| line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if compact.is_empty() {
        None
    } else {
        Some(compact)
    }
}

pub(super) fn compact_live_response_message(message: &str) -> Option<String> {
    let source = compact_live_response_source(message)?;
    let sentences = split_progress_sentences(&source);
    if sentences.len() <= 3 {
        return Some(sentences.join("\n"));
    }

    let next_markers = [
        "next ",
        "i will ",
        "i'll ",
        "then i will ",
        "then i'll ",
        "i am going to ",
    ];

    let mut selected_indices = vec![0];
    let mut next_step_idx = None;

    for (idx, sentence) in sentences.iter().enumerate().skip(1) {
        let lowered = sentence.to_ascii_lowercase();
        if next_step_idx.is_none()
            && next_markers
                .iter()
                .any(|marker| lowered.starts_with(marker))
        {
            next_step_idx = Some(idx);
            break;
        }
    }

    if let Some(idx) = next_step_idx {
        selected_indices.push(idx);
    }

    let mut idx = 1;
    while selected_indices.len() < 3 && idx < sentences.len() {
        if !selected_indices.contains(&idx) {
            selected_indices.push(idx);
        }
        idx += 1;
    }

    selected_indices.sort_unstable();
    Some(
        selected_indices
            .into_iter()
            .map(|idx| sentences[idx].clone())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub(super) fn parse_render_plan_block(
    message: &str,
) -> Option<(Vec<(String, String)>, Option<String>)> {
    let (start_tag, end_tag, start, end) = find_render_plan_block_bounds(message)
        .or_else(|| find_render_legacy_plan_block_bounds(message))?;
    if end <= start {
        return None;
    }

    let block = &message[start + start_tag.len()..end];
    let mut steps = Vec::new();
    for line in block.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(step) = parse_render_plan_step_line(line) {
            steps.push(step);
        }
    }

    if steps.is_empty() {
        if start_tag != "<proposed_plan>" {
            return None;
        }
        let fallback = block
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'))
            .unwrap_or("Implement proposed plan")
            .trim_matches(['*', '#', ' '])
            .to_string();
        steps.push(("pending".to_string(), fallback));
    }

    let explanation = message[end + end_tag.len()..].trim();
    Some((
        steps,
        (!explanation.is_empty()).then(|| explanation.to_string()),
    ))
}

pub(super) fn find_render_plan_block_bounds(
    message: &str,
) -> Option<(&'static str, &'static str, usize, usize)> {
    let start_tag = "<proposed_plan>";
    let end_tag = "</proposed_plan>";
    let start = message.find(start_tag)?;
    let end = message.find(end_tag)?;
    Some((start_tag, end_tag, start, end))
}

pub(super) fn find_render_legacy_plan_block_bounds(
    message: &str,
) -> Option<(&'static str, &'static str, usize, usize)> {
    let start_tag = "<plan>";
    let end_tag = "</plan>";
    let start = message.find(start_tag)?;
    let end = message.find(end_tag)?;
    Some((start_tag, end_tag, start, end))
}

pub(super) fn parse_render_plan_step_line(line: &str) -> Option<(String, String)> {
    if let Some(step) = line
        .strip_prefix("- [")
        .or_else(|| line.strip_prefix("* ["))
        .or_else(|| line.strip_prefix("• ["))
    {
        let Some((status, rest)) = step.split_once(']') else {
            return None;
        };
        let step = rest.trim();
        return (!step.is_empty()).then(|| (status.trim().to_string(), step.to_string()));
    }

    let step = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("• "))
        .or_else(|| {
            let (number, rest) = line.split_once(". ")?;
            number.chars().all(|ch| ch.is_ascii_digit()).then_some(rest)
        })?
        .trim();
    (!step.is_empty()).then(|| ("pending".to_string(), step.to_string()))
}
