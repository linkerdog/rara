use super::tasks::start_goal_continuation_task;
use crate::agent::Agent;
use crate::runtime_goals::{GoalStatus, RalphGoal};
use crate::tui::goal_ui::{self, GoalDialog, GoalUiAction};
use crate::tui::runtime_port::{RuntimeClientPort, RuntimeCommand};
use crate::tui::state::{Overlay, TuiApp};

pub(super) async fn handle_command(
    arg: Option<&str>,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) {
    app.goal = app.goal_handle.snapshot();
    let result: anyhow::Result<()> = async {
        match arg.unwrap_or("").trim() {
            "" => goal_ui::open(app, GoalDialog::Summary),
            "edit" => {
                if let Some(ticket) = app.goal_handle.resume_ticket() {
                    goal_ui::open(app, GoalDialog::Edit(ticket));
                } else {
                    app.push_notice("No goal to edit. Use /goal <objective> to create one.");
                }
            }
            "pause" => {
                let paused = app.goal_handle.mutate(|stored| {
                    if let Some(goal) = stored
                        .as_mut()
                        .filter(|goal| goal.status == GoalStatus::Pursuing)
                    {
                        goal.status = GoalStatus::Paused;
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                })?;
                app.goal = app.goal_handle.snapshot();
                app.push_notice(if paused {
                    "Goal paused. Use /goal resume to continue."
                } else {
                    "Goal is not currently pursuing; nothing to pause."
                });
            }
            "resume" => resume_goal_continuation(app, agent_slot, runtime_port).await?,
            "clear" => {
                app.pending_goal_resume = None;
                if app.goal.is_some() {
                    app.goal_handle.replace(None)?;
                    app.goal = None;
                    app.push_notice("Goal cleared.");
                } else {
                    app.push_notice("No active goal to clear.");
                }
            }
            objective => {
                let (objective, budget) =
                    parse_goal_objective_and_budget(objective).map_err(anyhow::Error::msg)?;
                if app
                    .goal
                    .as_ref()
                    .is_some_and(|goal| goal.status != GoalStatus::Complete)
                {
                    let ticket = app
                        .goal_handle
                        .resume_ticket()
                        .ok_or_else(|| anyhow::anyhow!("goal changed; open /goal again"))?;
                    goal_ui::open(
                        app,
                        GoalDialog::Replace {
                            ticket,
                            objective,
                            budget,
                        },
                    );
                } else {
                    app.goal_handle.mutate(|stored| {
                        anyhow::ensure!(
                            stored
                                .as_ref()
                                .is_none_or(|goal| goal.status == GoalStatus::Complete),
                            "an unfinished goal already exists"
                        );
                        *stored = Some(RalphGoal::new(objective, budget));
                        Ok(())
                    })?;
                    start_new_goal(app, agent_slot, runtime_port).await?;
                }
            }
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        report_error(app, error);
    }
}

pub(crate) async fn apply_dialog_action(
    action: GoalUiAction,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) {
    if app.overlay != Some(Overlay::Goal) {
        return;
    }
    if let GoalUiAction::Move(delta) = action {
        app.goal_ui.selected = (app.goal_ui.selected as i32 + delta).clamp(0, 1) as usize;
        return;
    }
    let Some(dialog) = app.goal_ui.dialog.take() else {
        return;
    };
    let input = app.goal_ui.input.trim().to_string();
    let selected = app.goal_ui.selected;
    if app.is_busy() && !matches!(dialog, GoalDialog::Summary) {
        app.goal_ui.dialog = Some(dialog);
        app.push_notice("Wait for the current task before changing the goal.");
        return;
    }
    if matches!(dialog, GoalDialog::Edit(_)) && input.is_empty() {
        app.goal_ui.dialog = Some(dialog);
        app.push_notice("Goal objective cannot be empty.");
        return;
    }
    app.dismiss_overlay();
    let result: anyhow::Result<()> = async {
        match dialog {
            GoalDialog::Summary => {}
            GoalDialog::Resume(ticket) => {
                if selected == 0 {
                    anyhow::ensure!(
                        app.goal_handle.matches_resume_ticket(&ticket),
                        "goal changed; open /goal again"
                    );
                    resume_goal_continuation(app, agent_slot, runtime_port).await?;
                }
            }
            GoalDialog::Edit(ticket) => {
                let waiting = app.pending_goal_resume.is_some();
                app.goal_handle.edit_objective(&ticket, input)?;
                app.goal = app.goal_handle.snapshot();
                if waiting {
                    crate::tui::goal_resume::arm_after_restore(app);
                }
                app.push_notice("Goal objective updated.");
            }
            GoalDialog::Replace {
                ticket,
                objective,
                budget,
            } => {
                if selected == 0 {
                    app.goal_handle
                        .replace_confirmed(&ticket, RalphGoal::new(objective, budget))?;
                    start_new_goal(app, agent_slot, runtime_port).await?;
                }
            }
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        report_error(app, error);
    }
}

fn report_error(app: &mut TuiApp, error: anyhow::Error) {
    log::warn!("Goal command failed: {error:#}");
    app.goal = app.goal_handle.snapshot();
    app.push_notice(format!("Goal command failed: {error:#}"));
}

async fn start_new_goal(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<()> {
    app.goal = app.goal_handle.snapshot();
    let goal = app
        .goal
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("goal was cleared before it could be started"))?;
    let mut notice = format!("Goal set: {}", goal.objective);
    if let Some(budget) = goal.token_budget {
        notice.push_str(&format!(" [budget: {budget} tokens]"));
    }
    if !app.is_busy() && app.active_pending_interaction().is_none() && agent_slot.is_some() {
        start_goal_follow_up(app, agent_slot, runtime_port).await?;
        notice.push_str(". Continuing active goal.");
    }
    app.push_notice(notice);
    Ok(())
}

async fn resume_goal_continuation(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<()> {
    if app.is_busy() {
        app.push_notice("A task is already running. Wait for it to finish before resuming a goal.");
        return Ok(());
    }
    if app.active_pending_interaction().is_some() {
        app.push_notice("Resolve the pending interaction before resuming a goal.");
        return Ok(());
    }
    if agent_slot.is_none() {
        app.push_notice("Goal resume is unavailable until the runtime agent is ready.");
        return Ok(());
    }

    let Some(mut goal) = app.goal_handle.snapshot() else {
        app.push_notice("No active goal to resume.");
        return Ok(());
    };
    let (previous_status, mut notice) = match goal.status {
        GoalStatus::Paused => (GoalStatus::Paused, "Goal resumed. Continuing active goal."),
        GoalStatus::Blocked => (
            GoalStatus::Blocked,
            "Goal resumed. The blocked-goal audit has restarted.",
        ),
        GoalStatus::Pursuing => (
            GoalStatus::Pursuing,
            "Goal resumed. Continuing interrupted work.",
        ),
        GoalStatus::Complete | GoalStatus::BudgetLimited => {
            app.push_notice("Goal is not paused or blocked; nothing to resume.");
            return Ok(());
        }
    };
    goal.status = if goal
        .token_budget
        .is_some_and(|budget| goal.tokens_used >= budget)
    {
        notice = "Goal budget exhausted. Wrapping up without new work.";
        GoalStatus::BudgetLimited
    } else {
        GoalStatus::Pursuing
    };
    app.goal_handle.replace(Some(goal))?;
    app.goal = app.goal_handle.snapshot();

    if let Err(error) = start_goal_follow_up(app, agent_slot, runtime_port).await {
        app.goal_handle.mutate(|stored| {
            if let Some(goal) = stored.as_mut() {
                goal.status = previous_status;
            }
            Ok(())
        })?;
        app.goal = app.goal_handle.snapshot();
        return Err(error);
    }
    app.push_notice(notice);
    Ok(())
}

pub(super) async fn start_goal_follow_up(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    runtime_port: Option<&dyn RuntimeClientPort>,
) -> anyhow::Result<()> {
    let goal = app
        .goal
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("goal follow-up requires an active goal"))?;
    let prompt = match goal.status {
        GoalStatus::Pursuing => crate::runtime_client::goal_continuation_prompt(goal),
        GoalStatus::BudgetLimited => crate::runtime_client::goal_budget_limit_prompt(goal),
        GoalStatus::Paused | GoalStatus::Blocked | GoalStatus::Complete => {
            anyhow::bail!("inactive goals cannot start a follow-up");
        }
    };

    if let Some(runtime_port) = runtime_port {
        runtime_port
            .send(RuntimeCommand::ContinueGoal {
                ticket: app
                    .goal_handle
                    .resume_ticket()
                    .ok_or_else(|| anyhow::anyhow!("goal follow-up requires a stored goal"))?,
                mode: crate::runtime_goals::GoalContinuationMode::Requested,
            })
            .await?;
    } else {
        let agent = agent_slot
            .take()
            .ok_or_else(|| anyhow::anyhow!("goal continuation requires a ready runtime agent"))?;
        start_goal_continuation_task(app, prompt, agent);
    }
    Ok(())
}

pub(super) fn parse_goal_objective_and_budget(
    input: &str,
) -> Result<(String, Option<u32>), String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Goal objective cannot be empty.".into());
    }

    if let Some(rest) = input.strip_prefix("--tokens ") {
        let (budget_raw, objective) = rest
            .trim()
            .split_once(char::is_whitespace)
            .ok_or_else(|| "Usage: /goal --tokens <N> <objective>.".to_string())?;
        let budget = parse_goal_token_budget(budget_raw)
            .ok_or_else(|| format!("Invalid goal token budget: {budget_raw}."))?;
        let objective = objective.trim();
        if objective.is_empty() {
            return Err("Goal objective cannot be empty.".into());
        }
        return Ok((objective.to_string(), Some(budget)));
    }

    if let Some((first, rest)) = input.split_once(char::is_whitespace)
        && first.bytes().all(|b| b.is_ascii_digit())
    {
        let budget = parse_goal_token_budget(first)
            .ok_or_else(|| format!("Invalid goal token budget: {first}."))?;
        let objective = rest.trim();
        if objective.is_empty() {
            return Err("Goal objective cannot be empty.".into());
        }
        return Ok((objective.to_string(), Some(budget)));
    }

    Ok((input.to_string(), None))
}

pub(super) fn parse_goal_token_budget(input: &str) -> Option<u32> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (number, multiplier) = match trimmed.as_bytes().last().copied() {
        Some(b'k') | Some(b'K') => (&trimmed[..trimmed.len() - 1], 1_000.0),
        Some(b'm') | Some(b'M') => (&trimmed[..trimmed.len() - 1], 1_000_000.0),
        _ => (trimmed, 1.0),
    };
    if number.is_empty() || number.starts_with('-') {
        return None;
    }

    let value = number.parse::<f64>().ok()? * multiplier;
    if !value.is_finite() || value <= 0.0 || value > u32::MAX as f64 {
        return None;
    }

    let budget = value.round() as u32;
    (budget > 0).then_some(budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::RuntimeSnapshot;
    use crate::tui::testing::TuiHarness;

    #[tokio::test]
    async fn goal_cleared_before_start_returns_a_recoverable_error() {
        let mut tui = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
        let mut agent = None;
        let error = start_new_goal(tui.app_mut(), &mut agent, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("goal was cleared"));
        assert!(tui.app().goal.is_none());
        assert!(!tui.app().is_busy());
        tui.expect_no_commands();
    }
}
