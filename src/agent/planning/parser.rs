use serde_json::Value;

use super::{PendingUserInput, PlanStep, PlanStepStatus};

pub(in crate::agent) fn parse_plan_block(text: &str) -> Option<(Vec<PlanStep>, Option<String>)> {
    let (start_tag, end_tag, start, end) =
        find_plan_block_bounds(text).or_else(|| find_legacy_plan_block_bounds(text))?;
    if end <= start {
        return None;
    }

    let block = &text[start + start_tag.len()..end];
    let trailing_explanation = text[end + end_tag.len()..].trim();
    if start_tag == "<proposed_plan>" && is_structured_proposed_plan(block) {
        let (steps, explanation) = parse_structured_proposed_plan(block, trailing_explanation);
        return (!steps.is_empty()).then_some((steps, explanation));
    }

    let mut steps = Vec::new();
    for line in block.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(step) = parse_plan_step_line(line) {
            steps.push(step);
        }
    }

    let mut explanation = trailing_explanation.to_string();
    if steps.is_empty() && start_tag == "<proposed_plan>" {
        let fallback = block
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'))
            .unwrap_or("Implement proposed plan");
        steps.push(PlanStep {
            step: fallback.trim_matches(['*', '#', ' ']).to_string(),
            status: PlanStepStatus::Pending,
        });
        if explanation.is_empty() {
            explanation = block.trim().to_string();
        }
    }

    Some((
        steps,
        (!explanation.is_empty()).then(|| explanation.to_string()),
    ))
}

pub(in crate::agent) fn parse_exit_plan_tool_input(
    input: &Value,
) -> Option<(Vec<PlanStep>, Option<String>)> {
    let proposed_plan = input.get("proposed_plan")?;
    let steps = proposed_plan
        .get("steps")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(parse_proposed_plan_step_value)
        .collect::<Vec<_>>();
    if steps.is_empty() {
        return None;
    }

    let mut explanation_lines = Vec::new();
    if let Some(summary) = proposed_plan
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        explanation_lines.push(format!("summary: {summary}"));
    }
    if let Some(validation) = proposed_plan.get("validation").and_then(Value::as_array) {
        let validation_lines = validation
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if !validation_lines.is_empty() {
            explanation_lines.push("validation:".to_string());
            explanation_lines.extend(validation_lines.into_iter().map(|line| format!("- {line}")));
        }
    }

    let explanation = explanation_lines.join("\n").trim().to_string();
    Some((steps, (!explanation.is_empty()).then_some(explanation)))
}

fn parse_proposed_plan_step_value(value: &Value) -> Option<PlanStep> {
    if let Some(step) = value
        .as_str()
        .map(str::trim)
        .filter(|step| !step.is_empty())
    {
        return Some(PlanStep {
            step: step.to_string(),
            status: PlanStepStatus::Pending,
        });
    }

    let step = value
        .get("step")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|step| !step.is_empty())?;
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .and_then(parse_plan_status)
        .unwrap_or(PlanStepStatus::Pending);
    Some(PlanStep {
        step: step.to_string(),
        status,
    })
}

fn parse_plan_status(status: &str) -> Option<PlanStepStatus> {
    match status.trim() {
        "pending" => Some(PlanStepStatus::Pending),
        "in_progress" => Some(PlanStepStatus::InProgress),
        "completed" => Some(PlanStepStatus::Completed),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProposedPlanSection {
    None,
    Steps,
    Validation,
}

fn is_structured_proposed_plan(block: &str) -> bool {
    block
        .lines()
        .map(str::trim)
        .any(|line| header_key(line) == Some("steps"))
}

fn parse_structured_proposed_plan(
    block: &str,
    trailing_explanation: &str,
) -> (Vec<PlanStep>, Option<String>) {
    let mut section = ProposedPlanSection::None;
    let mut steps = Vec::new();
    let mut explanation_lines = Vec::new();

    for line in block.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(key) = header_key(line) {
            match key {
                "steps" => {
                    section = ProposedPlanSection::Steps;
                    continue;
                }
                "validation" | "tests" => {
                    section = ProposedPlanSection::Validation;
                    explanation_lines.push("validation:".to_string());
                    continue;
                }
                "summary" | "title" => {
                    section = ProposedPlanSection::None;
                    if let Some(value) = header_value(line) {
                        explanation_lines.push(format!("{key}: {}", value.trim()));
                    }
                    continue;
                }
                _ => {}
            }
        }

        match section {
            ProposedPlanSection::Steps => {
                if let Some(step) = parse_plan_step_line(line) {
                    steps.push(step);
                }
            }
            ProposedPlanSection::Validation => {
                explanation_lines.push(line.to_string());
            }
            ProposedPlanSection::None => {
                explanation_lines.push(line.to_string());
            }
        }
    }

    if !trailing_explanation.is_empty() {
        if !explanation_lines.is_empty() {
            explanation_lines.push(String::new());
        }
        explanation_lines.push(trailing_explanation.to_string());
    }

    let explanation = explanation_lines.join("\n").trim().to_string();
    (steps, (!explanation.is_empty()).then_some(explanation))
}

fn header_key(line: &str) -> Option<&'static str> {
    let (key, _) = line.split_once(':')?;
    match key.trim().to_ascii_lowercase().as_str() {
        "steps" => Some("steps"),
        "validation" => Some("validation"),
        "tests" => Some("tests"),
        "summary" => Some("summary"),
        "title" => Some("title"),
        _ => None,
    }
}

fn header_value(line: &str) -> Option<&str> {
    line.split_once(':').map(|(_, value)| value)
}

fn find_plan_block_bounds(text: &str) -> Option<(&'static str, &'static str, usize, usize)> {
    let start_tag = "<proposed_plan>";
    let end_tag = "</proposed_plan>";
    let start = text.find(start_tag)?;
    let end = text.find(end_tag)?;
    Some((start_tag, end_tag, start, end))
}

fn find_legacy_plan_block_bounds(text: &str) -> Option<(&'static str, &'static str, usize, usize)> {
    let start_tag = "<plan>";
    let end_tag = "</plan>";
    let start = text.find(start_tag)?;
    let end = text.find(end_tag)?;
    Some((start_tag, end_tag, start, end))
}

pub(in crate::agent) fn has_unclosed_proposed_plan_block(text: &str) -> bool {
    let start_tag = "<proposed_plan>";
    let end_tag = "</proposed_plan>";
    let mut cursor = 0;
    let mut open_blocks = 0usize;

    loop {
        let next_start = text[cursor..].find(start_tag);
        let next_end = text[cursor..].find(end_tag);

        match (next_start, next_end) {
            (Some(start), Some(end)) if start < end => {
                open_blocks += 1;
                cursor += start + start_tag.len();
            }
            (Some(start), None) => {
                open_blocks += 1;
                cursor += start + start_tag.len();
            }
            (Some(_), Some(end)) => {
                open_blocks = open_blocks.saturating_sub(1);
                cursor += end + end_tag.len();
            }
            (None, Some(end)) => {
                open_blocks = open_blocks.saturating_sub(1);
                cursor += end + end_tag.len();
            }
            (None, None) => break,
        }
    }

    open_blocks > 0
}

fn parse_plan_step_line(line: &str) -> Option<PlanStep> {
    if let Some(rest) = line
        .strip_prefix("- [")
        .or_else(|| line.strip_prefix("* ["))
        .or_else(|| line.strip_prefix("• ["))
    {
        let (status, step) = rest.split_once("] ")?;
        let status = match status.trim() {
            "pending" => PlanStepStatus::Pending,
            "in_progress" => PlanStepStatus::InProgress,
            "completed" => PlanStepStatus::Completed,
            _ => return None,
        };
        let step = step.trim();
        return (!step.is_empty()).then(|| PlanStep {
            step: step.to_string(),
            status,
        });
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
    (!step.is_empty()).then(|| PlanStep {
        step: step.to_string(),
        status: PlanStepStatus::Pending,
    })
}

pub(in crate::agent) fn parse_request_user_input_block(text: &str) -> Option<PendingUserInput> {
    let start = text.find("<request_user_input>")?;
    let end = text.find("</request_user_input>")?;
    if end <= start {
        return None;
    }

    let block = &text[start + "<request_user_input>".len()..end];
    let mut question = None;
    let mut options = Vec::new();
    for line in block.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(value) = line.strip_prefix("question:") {
            question = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = line.strip_prefix("option:") {
            let value = value.trim();
            if let Some((label, description)) = value.split_once('|') {
                options.push((label.trim().to_string(), description.trim().to_string()));
            } else {
                options.push((value.to_string(), String::new()));
            }
        }
    }

    let mut note = text[end + "</request_user_input>".len()..].trim();
    note = note.strip_prefix("</proposed_plan>").unwrap_or(note).trim();
    note = note.strip_prefix("</plan>").unwrap_or(note).trim();
    let note = note.to_string();

    Some(PendingUserInput {
        question: question?,
        options,
        note: (!note.is_empty()).then_some(note),
    })
}
