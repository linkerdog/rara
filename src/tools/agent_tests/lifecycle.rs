use super::*;

#[test]
fn agent_token_budget_rejects_invalid_values() {
    let zero = parse_agent_token_budget(0).expect_err("zero should fail");
    assert!(matches!(zero, ToolError::InvalidInput(message) if message.contains("positive")));

    let oversized =
        parse_agent_token_budget(i64::from(u32::MAX) + 1).expect_err("oversized should fail");
    assert!(matches!(oversized, ToolError::InvalidInput(message) if message.contains("maximum")));
}

#[tokio::test]
async fn background_subagent_resume_returns_completed_summary_without_inline_sidechain() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let background_subagents = Arc::new(BackgroundSubAgentStore::default());
    let session_manager =
        Arc::new(SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"));
    let tool = ExploreAgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: session_manager.clone(),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: background_subagents.clone(),
    };
    let resume = SubAgentResumeTool {
        background_subagents: background_subagents.clone(),
        session_manager,
    };

    let mut progress = |_| {};
    let started = tool
        .call_with_context_events(
            json!({
                "instruction": "inspect this in the background",
                "run_in_background": true
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("background start");
    let agent_id = started["agent_id"].as_str().expect("agent_id");
    let child_session_id = started["session_id"].as_str().expect("session_id");
    assert_eq!(started["status"], "running");

    let mut completed = None;
    for _ in 0..20 {
        let status = resume
            .call_with_context_events(
                json!({ "agent_id": agent_id }),
                ToolCallContext::default().with_session_id("parent-session"),
                &mut |_| {},
            )
            .await
            .expect("resume status");
        if status["status"] != "running" {
            completed = Some(status);
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    let completed = completed.expect("background sub-agent completed");
    assert_eq!(completed["status"], "explored");
    assert!(
        completed["summary"]
            .as_str()
            .expect("summary")
            .starts_with("counted")
    );
    assert_eq!(completed["session_id"], child_session_id);

    let transcript_path = rara_dir
        .join("rollouts")
        .join("parent-session")
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"));
    let transcript = load_transcript(&transcript_path).expect("transcript");
    assert_eq!(transcript.parse_errors, 0);
    assert!(model_visible_messages(&transcript.entries).is_empty());
}

#[tokio::test]
async fn subagent_resume_reconnects_completed_sidechain_after_store_restart() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let live_store = Arc::new(BackgroundSubAgentStore::default());
    let session_manager =
        Arc::new(SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"));
    let tool = ExploreAgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: session_manager.clone(),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: live_store,
    };

    let mut progress = |_| {};
    let started = tool
        .call_with_context_events(
            json!({
                "instruction": "inspect this in the background",
                "run_in_background": true
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("background start");
    let agent_id = started["agent_id"].as_str().expect("agent_id");

    let reconnect_store = Arc::new(BackgroundSubAgentStore::default());
    let resume = SubAgentResumeTool {
        background_subagents: reconnect_store.clone(),
        session_manager: session_manager.clone(),
    };
    let list = SubAgentListTool {
        background_subagents: reconnect_store,
        session_manager,
    };

    let mut reconnected = None;
    for _ in 0..50 {
        let status = resume
            .call_with_context_events(
                json!({ "agent_id": agent_id }),
                ToolCallContext::default().with_session_id("parent-session"),
                &mut |_| {},
            )
            .await;
        if let Ok(status) = status {
            reconnected = Some(status);
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }

    let reconnected = reconnected.expect("durable sub-agent result");
    assert_eq!(reconnected["agent_id"], agent_id);
    assert_eq!(reconnected["status"], "explored");
    assert_eq!(reconnected["parent_session_id"], "parent-session");
    assert_eq!(reconnected["kind"], "reconnected");
    assert!(
        reconnected["summary"]
            .as_str()
            .expect("summary")
            .starts_with("counted")
    );

    let listed = list
        .call_with_context_events(
            json!({}),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut |_| {},
        )
        .await
        .expect("subagent list");
    let agents = listed["subagents"].as_array().expect("subagents");
    assert!(agents.iter().any(|record| record["agent_id"] == agent_id));
}

#[tokio::test]
async fn background_subagent_stop_marks_running_task_cancelled() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let background_subagents = Arc::new(BackgroundSubAgentStore::default());
    let session_manager =
        Arc::new(SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"));
    let tool = ExploreAgentTool {
        backend: Arc::new(SlowBackend),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: session_manager.clone(),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: background_subagents.clone(),
    };
    let stop = SubAgentStopTool {
        background_subagents: background_subagents.clone(),
    };
    let resume = SubAgentResumeTool {
        background_subagents: background_subagents.clone(),
        session_manager,
    };

    let mut progress = |_| {};
    let started = tool
        .call_with_context_events(
            json!({
                "instruction": "keep running until stopped",
                "run_in_background": true
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("background start");
    let agent_id = started["agent_id"].as_str().expect("agent_id");

    let stopped = stop
        .call_with_context_events(
            json!({ "agent_id": agent_id }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut |_| {},
        )
        .await
        .expect("stop sub-agent");
    assert_eq!(stopped["status"], "cancelled");
    assert_eq!(
        background_subagents.available_permits(),
        background_subagents.max_active_subagents() - 1,
        "cancellation must not release capacity before execution exits"
    );

    let resumed = resume
        .call_with_context_events(
            json!({ "agent_id": agent_id }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut |_| {},
        )
        .await
        .expect("resume cancelled sub-agent");
    assert_eq!(resumed["status"], "cancelled");
    assert!(resumed["finished_at"].as_u64().is_some());

    tokio::time::timeout(Duration::from_secs(1), async {
        while background_subagents.available_permits()
            != background_subagents.max_active_subagents()
        {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled execution should release its permit after exiting");
}

#[test]
fn background_subagent_store_prunes_old_completed_records() {
    let store = BackgroundSubAgentStore::default();
    {
        let mut inner = store.inner.lock().expect("store");
        for idx in 0..(BACKGROUND_SUBAGENT_COMPLETED_RETENTION + 3) {
            let agent_id = format!("agent-{idx}");
            inner.tasks.insert(
                agent_id.clone(),
                BackgroundSubAgentRecord {
                    agent_id,
                    path: format!("/root/agent-{idx}"),
                    session_id: format!("session-{idx}"),
                    name: None,
                    provider: None,
                    model: None,
                    progress: SubagentProgress::new("test".to_string()),
                    kind: "general",
                    parent_session_id: None,
                    status: "done".to_string(),
                    summary: Some(format!("summary {idx}")),
                    error: None,
                    persistence_error: None,
                    plan: None,
                    plan_explanation: None,
                    request_user_input: None,
                    started_at: idx as u64,
                    finished_at: Some(idx as u64),
                },
            );
        }
    }

    store.finish(
        &format!("agent-{}", BACKGROUND_SUBAGENT_COMPLETED_RETENTION + 2),
        &Err(ToolError::ExecutionFailed("refresh latest".to_string())),
        AgentResultDelivery::Direct,
    );

    let records = store.list().expect("records");
    assert_eq!(records.len(), BACKGROUND_SUBAGENT_COMPLETED_RETENTION);
    assert!(records.iter().any(|record| record.agent_id == "agent-66"));
    assert!(!records.iter().any(|record| record.agent_id == "agent-0"));
}

#[tokio::test]
async fn background_plan_agent_resume_returns_plan_state() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let background_subagents = Arc::new(BackgroundSubAgentStore::default());
    let session_manager =
        Arc::new(SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"));
    let tool = PlanAgentTool {
        backend: Arc::new(PlanStateBackend),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: session_manager.clone(),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: background_subagents.clone(),
    };
    let resume = SubAgentResumeTool {
        background_subagents,
        session_manager,
    };

    let mut progress = |_| {};
    let started = tool
        .call_with_context_events(
            json!({
                "instruction": "plan this in the background",
                "run_in_background": true
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("background start");
    let agent_id = started["agent_id"].as_str().expect("agent_id");

    let mut completed = None;
    for _ in 0..20 {
        let status = resume
            .call_with_context_events(
                json!({ "agent_id": agent_id }),
                ToolCallContext::default().with_session_id("parent-session"),
                &mut |_| {},
            )
            .await
            .expect("resume status");
        if status["status"] != "running" {
            completed = Some(status);
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    let completed = completed.expect("background plan sub-agent completed");
    assert_eq!(completed["status"], "planned");
    let steps = completed["plan"].as_array().expect("plan steps");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["step"], "Inspect subagent restore");
    assert_eq!(steps[1]["status"], "in_progress");
}

#[tokio::test]
async fn plan_agent_writes_parent_scoped_sidechain_transcript() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = PlanAgentTool {
        backend: Arc::new(PlanStateBackend),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
    };

    let mut progress = |_| {};
    let result = tool
        .call_with_context_events(
            json!({ "instruction": "plan this task" }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("plan_agent result");

    let agent_id = result["agent_id"].as_str().expect("agent_id");
    let child_session_id = result["session_id"].as_str().expect("session_id");
    assert!(agent_id.starts_with("plan-"));
    let transcript_path = rara_dir
        .join("rollouts")
        .join("parent-session")
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"));
    let transcript = load_transcript(&transcript_path).expect("transcript");
    assert_eq!(transcript.parse_errors, 0);
    assert!(model_visible_messages(&transcript.entries).is_empty());

    let events =
        thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
            .expect("rollout events");
    assert!(matches!(
        events.as_slice(),
        [PersistedStructuredRolloutEvent::SpawnAgent {
            agent_id: event_agent_id,
            name: None,
            child_session_id: event_child_session_id,
            status,
            ..
        }] if event_agent_id == agent_id
            && event_child_session_id == child_session_id
            && status == "planned"
    ));

    let state_db = StateDb::new_for_root_dir(rara_dir).expect("state db");
    let thread_store = ThreadStore::new(tool.session_manager.as_ref(), &state_db);
    let child = thread_store
        .load_thread(child_session_id)
        .expect("child thread");
    assert_eq!(
        child.provenance.metadata_source,
        ThreadMetadataSource::StructuredMetadata
    );
    assert_eq!(child.plan_steps.len(), 2);
    assert_eq!(child.plan_steps[0].step, "Inspect subagent restore");
    assert_eq!(child.plan_steps[0].status, "pending");
    assert_eq!(child.plan_steps[1].step, "Persist child state");
    assert_eq!(child.plan_steps[1].status, "in_progress");
}

#[tokio::test]
async fn subagent_without_parent_context_does_not_write_sidechain() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = ExploreAgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
    };

    let result = tool
        .call(json!({ "instruction": "look around" }))
        .await
        .expect("explore result");

    assert!(
        result["agent_id"]
            .as_str()
            .expect("agent_id")
            .starts_with("explore-")
    );
    assert!(!rara_dir.join("rollouts").join("subagents").exists());
    assert!(
        thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
            .expect("rollout events")
            .is_empty()
    );
}

#[tokio::test]
async fn subagent_returns_result_when_sidechain_persistence_fails() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let rollouts_dir = rara_dir.join("rollouts");
    std::fs::create_dir_all(&rollouts_dir).expect("rollouts");
    std::fs::write(rollouts_dir.join("blocked-parent"), b"not a directory").expect("blocking file");
    let tool = ExploreAgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
    };

    let mut progress = |_| {};
    let result = tool
        .call_with_context_events(
            json!({ "instruction": "look around" }),
            ToolCallContext::default().with_session_id("blocked-parent"),
            &mut progress,
        )
        .await
        .expect("explore result");

    assert_eq!(result["status"], "explored");
    assert!(
        result["summary"]
            .as_str()
            .expect("summary")
            .starts_with("counted")
    );
    assert!(
        !result["persistence_error"]
            .as_str()
            .expect("persistence error")
            .is_empty()
    );
}

#[tokio::test]
async fn team_create_rejects_too_many_tasks() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(MockLlm),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
        workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        task_list_id: test_task_list_id(),
    };

    let tasks = (0..9)
        .map(|idx| json!({ "instruction": format!("task {idx}") }))
        .collect::<Vec<_>>();
    let err = tool
        .call(json!({ "tasks": tasks }))
        .await
        .expect_err("too many tasks");

    assert!(matches!(err, ToolError::InvalidInput(message) if message.contains("at most 8 items")));
}
