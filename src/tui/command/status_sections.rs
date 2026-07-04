fn todo_summary_line(app: &TuiApp) -> String {
    let summary = &app.snapshot.todo.summary;
    if summary.total == 0 {
        return "none".to_string();
    }
    let active = summary
        .active_label
        .as_deref()
        .or(summary.active_item.as_deref())
        .unwrap_or("-");
    format!(
        "{} total, {} pending, {} in_progress, {} completed, {} cancelled, active={}",
        summary.total,
        summary.pending,
        summary.in_progress,
        summary.completed,
        summary.cancelled,
        truncate_preview(active, 80)
    )
}

fn shared_task_summary_line(app: &TuiApp) -> String {
    let tasks = &app.snapshot.shared_tasks;
    if let Some(error) = tasks.error.as_deref() {
        return format!(
            "list={} error={}",
            tasks.task_list_id,
            truncate_preview(error, 80)
        );
    }
    if tasks.total == 0 {
        return format!("list={} none", tasks.task_list_id);
    }
    format!(
        "list={} {} total, {} pending, {} in_progress, {} completed, {} unblocked, {} owned",
        tasks.task_list_id,
        tasks.total,
        tasks.pending,
        tasks.in_progress,
        tasks.completed,
        tasks.unblocked,
        tasks.owned,
    )
}

fn render_todo_context(app: &TuiApp) -> String {
    let summary = &app.snapshot.todo.summary;
    if summary.total == 0 {
        return "Todo\n  artifact: -\n  items: none".to_string();
    }
    let artifact = app.snapshot.todo_artifact_path.as_deref().unwrap_or("-");
    let updated_at = app
        .snapshot
        .todo
        .updated_at
        .map(format_unix_timestamp_utc)
        .unwrap_or_else(|| "-".to_string());
    let active = summary.active_item.as_deref();
    let items = if app.snapshot.todo.items.is_empty() {
        "  items: none".to_string()
    } else {
        let rendered_items = app
            .snapshot
            .todo
            .items
            .iter()
            .take(8)
            .map(|(id, status, content)| {
                let suffix = if active == Some(content.as_str()) {
                    format!("{id}, active")
                } else {
                    id.to_string()
                };
                format!(
                    "    - [{status}] {} ({suffix})",
                    truncate_preview(content, 100)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let omitted = app.snapshot.todo.items.len().saturating_sub(8);
        if omitted == 0 {
            format!("  items:\n{rendered_items}")
        } else {
            format!("  items:\n{rendered_items}\n    ... {omitted} more")
        }
    };
    format!(
        "Todo\n  artifact: {artifact}\n  updated_at: {updated_at}\n  total: {}  pending: {}  in_progress: {}  completed: {}  cancelled: {}\n{}",
        summary.total,
        summary.pending,
        summary.in_progress,
        summary.completed,
        summary.cancelled,
        items,
    )
}

fn render_planning_lifecycle_context(app: &TuiApp) -> String {
    let lifecycle = &app.snapshot.planning_lifecycle;
    let tool_line = lifecycle
        .tool_use_id
        .as_ref()
        .map(|tool_use_id| format!("\n  exit_plan_tool: {tool_use_id}"))
        .unwrap_or_default();
    format!(
        "Planning Lifecycle\n  plan_path: {}\n  approval_status: {}\n  pending_age: {}\n  last_decision: {}\n  approved_plan_revision: {}{}",
        lifecycle.plan_path.as_deref().unwrap_or("-"),
        lifecycle.approval_status.label(),
        lifecycle.pending_age_label(),
        lifecycle.last_decision_label(),
        lifecycle.approved_plan_revision_label(),
        tool_line,
    )
}

fn render_shared_tasks_context(app: &TuiApp) -> String {
    let tasks = &app.snapshot.shared_tasks;
    if let Some(error) = tasks.error.as_deref() {
        return format!(
            "Shared Tasks\n  list: {}\n  error: {}",
            tasks.task_list_id,
            truncate_preview(error, 120)
        );
    }
    if tasks.total == 0 {
        return format!(
            "Shared Tasks\n  list: {}\n  tasks: none",
            tasks.task_list_id
        );
    }
    let rendered_items = tasks
        .items
        .iter()
        .take(8)
        .map(|task| {
            let owner = task.owner.as_deref().unwrap_or("-");
            let blockers = if task.blocked_by.is_empty() {
                "-".to_string()
            } else {
                task.blocked_by.join(",")
            };
            format!(
                "    - [{}] #{} rev={} owner={} blockedBy={} {}",
                task.status,
                task.id,
                task.revision,
                owner,
                blockers,
                truncate_preview(task.subject.as_str(), 90)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let omitted = tasks.items.len().saturating_sub(8);
    let omitted = if omitted == 0 {
        String::new()
    } else {
        format!("\n    ... {omitted} more")
    };
    format!(
        "Shared Tasks\n  list: {}\n  total: {}  pending: {}  in_progress: {}  completed: {}  unblocked: {}  owned: {}\n  tasks:\n{}{}",
        tasks.task_list_id,
        tasks.total,
        tasks.pending,
        tasks.in_progress,
        tasks.completed,
        tasks.unblocked,
        tasks.owned,
        rendered_items,
        omitted,
    )
}

fn format_unix_timestamp_utc(timestamp: i64) -> String {
    let format =
        time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second] UTC");

    OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()
        .and_then(|dt| dt.format(&format).ok())
        .unwrap_or_else(|| "invalid timestamp".to_string())
}
