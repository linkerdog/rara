use super::*;

#[tokio::test]
async fn accounting_failure_keeps_the_runtime_agent_and_stops_automatic_continuation() {
    let temp = tempdir().expect("tempdir");
    let db = Arc::new(
        rara_state::state_db::StateDb::new_for_root_dir(temp.path().join("state"))
            .expect("state db"),
    );
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.snapshot.session_id = "failed-turn-thread".into();
    app.attach_state_db(db.clone());
    install_runtime_services(&mut app);
    let goal = RalphGoal::new("preserve goal after write failure".into(), None);
    app.goal_handle
        .replace(Some(goal.clone()))
        .expect("seed goal");
    app.goal = Some(goal.clone());
    let connection = rusqlite::Connection::open(db.path()).expect("independent connection");
    connection
        .execute_batch(
            "CREATE TRIGGER reject_goal_write BEFORE INSERT ON goals
        BEGIN SELECT RAISE(FAIL, 'injected accounting failure'); END;",
        )
        .expect("failure trigger");
    let mut agent = create_test_agent(&temp);
    agent.total_input_tokens = 25;
    install_completed_query_task(&mut app, agent, Ok(()));
    let mut slot = None;
    for _ in 0..20 {
        finish_running_task_if_ready(&mut app, &mut slot)
            .await
            .expect("finish task");
        if slot.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        slot.is_some(),
        "runtime agent must not be dropped on write failure"
    );
    assert!(
        app.bottom_pane.running_task.is_none(),
        "must not launch a continuation"
    );
    assert_eq!(app.goal_handle.snapshot(), Some(goal));
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .expect("failure notice")
            .contains("continuation stopped")
    );
}

#[test]
fn goal_turn_accounting_persists_budget_limit_usage_and_creation_time() {
    let temp = tempdir().expect("tempdir");
    let db = Arc::new(
        rara_state::state_db::StateDb::new_for_root_dir(temp.path().join("state"))
            .expect("state db"),
    );
    let store = Arc::new(crate::runtime_goals::GoalStore::default());
    store
        .restore_for_thread("accounted-thread", db.clone())
        .expect("bind");
    let mut goal = RalphGoal::new("preserve budget accounting".into(), Some(100));
    goal.tokens_used = 90;
    goal.turns_completed = 2;
    goal.created_at_epoch_seconds = 1_234_567_890;
    store.replace(Some(goal.clone())).expect("seed goal");
    let mut agent = create_test_agent(&temp);
    agent.total_input_tokens = 25;
    let continuation =
        crate::runtime_client::RuntimeClient::continue_goal(&store, &agent, 10, false, false)
            .expect("account turn");
    assert!(matches!(
        continuation,
        crate::runtime_client::GoalContinuation::BudgetLimited { .. }
    ));
    goal.status = GoalStatus::BudgetLimited;
    goal.tokens_used = 105;
    goal.turns_completed = 3;
    let mut fresh = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("fresh app");
    fresh.snapshot.session_id = "accounted-thread".into();
    fresh.attach_state_db(db);
    assert_eq!(fresh.goal, Some(goal));
}

#[test]
fn failed_goal_accounting_returns_an_error_without_a_continuation() {
    let temp = tempdir().expect("tempdir");
    let db = Arc::new(
        rara_state::state_db::StateDb::new_for_root_dir(temp.path().join("state"))
            .expect("state db"),
    );
    let store = Arc::new(crate::runtime_goals::GoalStore::default());
    store
        .restore_for_thread("accounted-thread", db.clone())
        .expect("bind");
    let goal = RalphGoal::new("preserve committed usage".into(), None);
    store.replace(Some(goal.clone())).expect("seed goal");
    let connection = rusqlite::Connection::open(db.path()).expect("independent connection");
    connection
        .execute_batch(
            "CREATE TRIGGER reject_goal_write BEFORE INSERT ON goals
        BEGIN SELECT RAISE(FAIL, 'injected accounting failure'); END;",
        )
        .expect("failure trigger");
    let mut agent = create_test_agent(&temp);
    agent.total_input_tokens = 25;
    assert!(
        crate::runtime_client::RuntimeClient::continue_goal(&store, &agent, 10, false, false)
            .is_err()
    );
    assert_eq!(store.snapshot(), Some(goal));
}

#[test]
fn goal_continuation_prompt_contains_budget_and_goal_status_rules() {
    let mut goal = RalphGoal::new("ship Codex goal parity".to_string(), Some(10_000));
    goal.tokens_used = 2_500;
    goal.turns_completed = 2;

    let prompt = goal_continuation_prompt(&goal);

    assert!(prompt.contains("<untrusted_objective>"));
    assert!(prompt.contains("ship Codex goal parity"));
    assert!(prompt.contains("Tokens used: 2500"));
    assert!(prompt.contains("Token budget: 10000"));
    assert!(prompt.contains("Tokens remaining: 7500"));
    assert!(prompt.contains("call update_goal with status \"complete\""));
    assert!(prompt.contains("at least three consecutive goal turns"));
    assert!(prompt.contains("status \"blocked\""));
    assert!(prompt.contains("After marking a goal blocked"));
}

#[test]
fn goal_budget_limit_prompt_asks_for_wrap_up_without_new_work() {
    let mut goal = RalphGoal::new("finish the migration".to_string(), Some(100));
    goal.tokens_used = 100;

    let prompt = goal_budget_limit_prompt(&goal);

    assert!(prompt.contains("has reached its token budget"));
    assert!(prompt.contains("Do not start new substantive work"));
    assert!(prompt.contains("finish the migration"));
    assert!(prompt.contains("Token budget: 100"));
}

#[tokio::test]
async fn pursuing_goal_continues_without_a_hidden_completion_classifier() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    install_runtime_services(&mut app);
    let goal = RalphGoal::new("run the missing test".to_string(), None);
    app.goal = Some(goal.clone());
    app.goal_handle.replace(Some(goal)).expect("seed goal");

    let mut agent = create_test_agent_with_backend(&temp, Arc::new(PlainAnswerBackend));
    agent.total_input_tokens = 10;
    install_completed_query_task(&mut app, agent, Ok(()));

    let mut agent_slot = None;
    for _ in 0..20 {
        finish_running_task_if_ready(&mut app, &mut agent_slot)
            .await
            .expect("finish task");
        if app.bottom_pane.running_task.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(app.bottom_pane.running_task.is_some());
    assert_eq!(
        app.goal.as_ref().map(|goal| goal.status),
        Some(GoalStatus::Pursuing)
    );
    assert_eq!(
        app.goal_handle.snapshot().as_ref().map(|goal| goal.status),
        Some(GoalStatus::Pursuing)
    );
    assert!(
        app.committed_turns
            .iter()
            .flat_map(|turn| turn.entries.iter())
            .all(|entry| !(entry.role == "System" && entry.message.starts_with("no:")))
    );

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[test]
fn merge_rebuilt_agent_preserves_session_and_turn_state() {
    let temp = tempdir().unwrap();
    let workspace_root = temp.path().join("workspace");
    let rara_dir = workspace_root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

    let workspace = Arc::new(WorkspaceMemory::from_paths(
        workspace_root.clone(),
        rara_dir.clone(),
    ));
    let session_manager = Arc::new(SessionManager {
        storage_dir: rara_dir.join("rollouts"),
        legacy_storage_dir: rara_dir.join("sessions"),
    });
    let backend = Arc::new(crate::llm::MockLlm);

    let mut previous = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager.clone(),
        workspace.clone(),
    );
    previous.session_id = "session-keep".to_string();
    previous.history.push(Message {
        role: "user".into(),
        content: json!([{"type":"text","text":"keep history"}]),
    });
    previous.total_input_tokens = 123;
    previous.total_output_tokens = 45;
    previous.total_cache_hit_tokens = 90;
    previous.total_cache_miss_tokens = 10;
    previous.execution_mode = AgentExecutionMode::Plan;
    previous.bash_approval_mode = BashApprovalMode::Suggestion;
    previous.set_full_access_mode(true);
    previous.approved_bash_prefixes = vec!["git push".to_string()];
    previous.current_plan = vec![PlanStep {
        step: "Keep session continuity".into(),
        status: PlanStepStatus::InProgress,
    }];
    previous.plan_explanation = Some("Do not reset the session during model switch.".into());
    previous.compact_state.estimated_history_tokens = 1_200;
    previous.compact_state.context_window_tokens = Some(8_192);
    previous.compact_state.compact_threshold_tokens = 7_000;
    previous.compact_state.reserved_output_tokens = 1_024;
    previous.compact_state.compaction_count = 2;
    previous.compact_state.last_compaction_before_tokens = Some(5_000);
    previous.compact_state.last_compaction_after_tokens = Some(2_100);
    previous.compact_state.last_compaction_recent_files = vec!["src/main.rs".into()];
    previous.compact_state.last_compaction_boundary = Some(crate::agent::CompactBoundaryMetadata {
        version: 1,
        before_tokens: 5_000,
        recent_file_count: 1,
    });
    previous.set_prompt_config(PromptRuntimeConfig {
        append_system_prompt: Some("keep appendix".to_string()),
        warnings: vec!["missing custom prompt".to_string()],
        ..PromptRuntimeConfig::default()
    });

    let mut rebuilt = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("other-memory").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    rebuilt.compact_state.context_window_tokens = Some(200_000);
    rebuilt.compact_state.compact_threshold_tokens = 180_000;
    rebuilt.compact_state.reserved_output_tokens = 8_192;

    let merged = merge_rebuilt_agent(rebuilt, previous);

    assert_eq!(merged.session_id, "session-keep");
    assert_eq!(merged.history.len(), 1);
    assert_eq!(merged.total_input_tokens, 123);
    assert_eq!(merged.total_output_tokens, 45);
    assert_eq!(merged.total_cache_hit_tokens, 90);
    assert_eq!(merged.total_cache_miss_tokens, 10);
    assert_eq!(merged.execution_mode, AgentExecutionMode::Plan);
    assert_eq!(merged.bash_approval_mode, BashApprovalMode::Suggestion);
    assert!(merged.full_access_mode);
    assert_eq!(merged.approved_bash_prefixes, vec!["git push".to_string()]);
    assert_eq!(merged.current_plan.len(), 1);
    assert_eq!(merged.compact_state.estimated_history_tokens, 1_200);
    assert_eq!(merged.compact_state.compaction_count, 2);
    assert_eq!(
        merged.compact_state.last_compaction_before_tokens,
        Some(5_000)
    );
    assert_eq!(
        merged.compact_state.last_compaction_after_tokens,
        Some(2_100)
    );
    assert_eq!(
        merged.compact_state.last_compaction_recent_files,
        vec!["src/main.rs".to_string()]
    );
    assert_eq!(merged.compact_state.context_window_tokens, Some(200_000));
    assert_eq!(merged.compact_state.compact_threshold_tokens, 180_000);
    assert_eq!(merged.compact_state.reserved_output_tokens, 8_192);
    assert_eq!(
        merged.prompt_config().append_system_prompt.as_deref(),
        Some("keep appendix")
    );
    assert_eq!(
        merged.prompt_config().warnings,
        vec!["missing custom prompt".to_string()]
    );
}
