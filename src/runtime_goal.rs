use crate::agent::{Agent, Message};
use crate::tui::state::RalphGoal;

const GOAL_EVALUATOR_INSTRUCTIONS: &str = "\
You are a goal completion evaluator. Decide whether the active goal is fully satisfied by the current transcript.

Output exactly one line:
- yes
- no: <one-sentence reason>

Rules:
- Answer yes only when the goal is complete, verified, and no required follow-up remains.
- Answer no when evidence is missing, verification is incomplete, or more work is still required.
- Do not include markdown, JSON, or extra explanation.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GoalEvaluation {
    Complete,
    Continue { reason: String },
}

pub(crate) async fn evaluate_goal_completion(agent: &Agent, goal: &RalphGoal) -> GoalEvaluation {
    let condition = goal
        .condition
        .as_deref()
        .filter(|condition| !condition.trim().is_empty())
        .unwrap_or(goal.objective.as_str());
    let messages = build_goal_evaluator_messages(condition, &agent.history);
    match agent
        .llm_backend
        .classify(GOAL_EVALUATOR_INSTRUCTIONS, messages.as_slice())
        .await
    {
        Ok(raw) => parse_goal_evaluation(raw.as_str()),
        Err(err) => GoalEvaluation::Continue {
            reason: format!("goal evaluator unavailable: {err}"),
        },
    }
}

fn build_goal_evaluator_messages(condition: &str, history: &[Message]) -> Vec<Message> {
    let recent = history
        .iter()
        .rev()
        .take(16)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(render_message_for_evaluator)
        .collect::<Vec<_>>()
        .join("\n");

    vec![Message {
        role: "user".to_string(),
        content: serde_json::Value::String(format!(
            "Goal:\n{condition}\n\nRecent transcript:\n{recent}\n\nIs the goal satisfied?"
        )),
    }]
}

fn render_message_for_evaluator(message: &Message) -> String {
    let text = message
        .content
        .as_str()
        .map(ToString::to_string)
        .unwrap_or_else(|| message.content.to_string());
    format!("{}: {}", message.role, collapse_ws(text.as_str()))
}

fn collapse_ws(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn parse_goal_evaluation(raw: &str) -> GoalEvaluation {
    let answer = raw.trim();
    let lower = answer.to_ascii_lowercase();
    if lower == "yes" || lower.starts_with("yes.") || lower.starts_with("yes\n") {
        return GoalEvaluation::Complete;
    }

    if lower == "no" {
        return GoalEvaluation::Continue {
            reason: "goal not yet complete".to_string(),
        };
    }

    for prefix in ["no:", "no -", "no --"] {
        if lower.starts_with(prefix) {
            let reason = answer[prefix.len()..].trim();
            return GoalEvaluation::Continue {
                reason: non_empty_reason(reason),
            };
        }
    }

    GoalEvaluation::Continue {
        reason: format!(
            "goal evaluator returned an unrecognized answer: {}",
            truncate_for_prompt(answer)
        ),
    }
}

fn non_empty_reason(reason: &str) -> String {
    if reason.trim().is_empty() {
        "goal not yet complete".to_string()
    } else {
        reason.trim().to_string()
    }
}

fn truncate_for_prompt(value: &str) -> String {
    const MAX_CHARS: usize = 240;
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yes_as_complete() {
        assert_eq!(parse_goal_evaluation("yes"), GoalEvaluation::Complete);
        assert_eq!(parse_goal_evaluation("Yes."), GoalEvaluation::Complete);
    }

    #[test]
    fn parses_no_reason_as_continuation() {
        assert_eq!(
            parse_goal_evaluation("no: tests have not run yet"),
            GoalEvaluation::Continue {
                reason: "tests have not run yet".to_string()
            }
        );
    }

    #[test]
    fn unrecognized_answer_continues_goal() {
        assert!(matches!(
            parse_goal_evaluation("maybe after another check"),
            GoalEvaluation::Continue { reason } if reason.contains("unrecognized answer")
        ));
    }
}
