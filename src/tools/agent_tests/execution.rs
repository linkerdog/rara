use super::*;

#[tokio::test]
async fn team_create_runs_real_subagents_in_order() {
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

    let result = tool
        .call(json!({
            "tasks": [
                {
                    "name": "research",
                    "kind": "general",
                    "instruction": "summarize one"
                },
                {
                    "name": "inspect",
                    "kind": "explore",
                    "instruction": "summarize two"
                }
            ]
        }))
        .await
        .expect("team_create result");
    let results = result["team_results"]
        .as_array()
        .expect("team_results array");

    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["name"], "research");
    assert_eq!(results[0]["status"], "done");
    assert_eq!(results[0]["summary"], "Mock Response: summarize one");
    assert_eq!(results[1]["name"], "inspect");
    assert_eq!(results[1]["status"], "explored");
    assert_eq!(results[1]["summary"], "Mock Response: summarize two");
    assert_ne!(results[0]["status"], "mocked_done");
}

#[tokio::test]
async fn team_create_validates_all_tasks_before_running_subagents() {
    let calls = Arc::new(AtomicUsize::new(0));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(CountingBackend {
            calls: calls.clone(),
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
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        task_list_id: test_task_list_id(),
    };

    let err = tool
        .call(json!({
            "tasks": [
                {
                    "name": "valid",
                    "instruction": "should not run"
                },
                {
                    "name": "invalid"
                }
            ]
        }))
        .await
        .expect_err("invalid task");

    assert!(matches!(err, ToolError::InvalidInput(message) if message == "tasks[1].instruction"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn team_create_rejects_non_string_kind_before_running_subagents() {
    let calls = Arc::new(AtomicUsize::new(0));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(CountingBackend {
            calls: calls.clone(),
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
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        task_list_id: test_task_list_id(),
    };

    let err = tool
        .call(json!({
            "tasks": [
                {
                    "instruction": "should not run",
                    "kind": 1
                }
            ]
        }))
        .await
        .expect_err("invalid kind");

    assert!(
        matches!(err, ToolError::InvalidInput(message) if message == "tasks[0].kind must be a string")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn team_create_rejects_unstable_explicit_name_before_running_subagents() {
    let calls = Arc::new(AtomicUsize::new(0));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
        backend: Arc::new(CountingBackend {
            calls: calls.clone(),
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
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        task_list_id: test_task_list_id(),
    };

    let err = tool
        .call(json!({
            "tasks": [
                {
                    "name": "!!!",
                    "instruction": "should not run"
                }
            ]
        }))
        .await
        .expect_err("invalid name");

    assert!(matches!(err, ToolError::InvalidInput(message)
                if message == "tasks[0].name must normalize to a non-empty agent id label"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn spawn_agent_rejects_name_that_normalizes_empty_before_running_subagent() {
    let calls = Arc::new(AtomicUsize::new(0));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = AgentTool {
        backend: Arc::new(CountingBackend {
            calls: calls.clone(),
        }),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
    };

    let err = tool
        .call(json!({
            "name": "!!!",
            "instruction": "should not run"
        }))
        .await
        .expect_err("invalid name");

    assert!(matches!(err, ToolError::InvalidInput(message)
                if message == "name must normalize to a non-empty agent id label"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn team_create_limits_concurrent_subagents() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let background_subagents = Arc::new(BackgroundSubAgentStore::default());
    let active_capacity = background_subagents.max_active_subagents();
    let tool = TeamCreateTool {
        backend: Arc::new(PeakBackend {
            in_flight,
            peak: peak.clone(),
            first_wave_arrivals: Arc::new(AtomicUsize::new(0)),
            first_wave_size: active_capacity,
            first_wave: Arc::new(Barrier::new(active_capacity)),
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
        background_subagents,
        task_list_id: test_task_list_id(),
    };
    let tasks = (0..8)
        .map(|idx| json!({ "kind": "general", "instruction": format!("task {idx}") }))
        .collect::<Vec<_>>();
    let result = tool
        .call(json!({ "tasks": tasks }))
        .await
        .expect("team_create result");

    assert_eq!(result["team_results"].as_array().expect("results").len(), 8);
    let observed_peak = peak.load(Ordering::SeqCst);
    assert_eq!(observed_peak, active_capacity);
}

#[tokio::test]
async fn team_create_writes_parent_scoped_sidechain_transcripts() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = TeamCreateTool {
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
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        task_list_id: test_task_list_id(),
    };

    let mut progress = |_| {};
    let result = tool
        .call_with_context_events(
            json!({
                "tasks": [
                    {
                        "name": "Review Worker",
                        "kind": "general",
                        "instruction": "summarize this task"
                    }
                ]
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("team_create result");

    let item = &result["team_results"][0];
    let agent_id = item["agent_id"].as_str().expect("agent_id");
    let child_session_id = item["session_id"].as_str().expect("session_id");
    let result_status = item["status"].as_str().expect("status");
    let result_summary = item["summary"].as_str().expect("summary");
    assert!(agent_id.starts_with("review-worker-"));
    let transcript_path = rara_dir
        .join("rollouts")
        .join("parent-session")
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"));
    let transcript = load_transcript(&transcript_path).expect("transcript");

    assert_eq!(transcript.parse_errors, 0);
    assert!(model_visible_messages(&transcript.entries).is_empty());
    assert!(matches!(
        &transcript.entries[0],
        crate::session_transcript::SessionTranscriptEntry::SessionMeta {
            session_id,
            parent_session_id: Some(parent),
            agent_id: Some(entry_agent_id),
            is_sidechain: true,
            ..
        } if session_id == child_session_id
            && parent == "parent-session"
            && entry_agent_id == agent_id
    ));

    let events =
        thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
            .expect("rollout events");
    assert!(matches!(
        events.as_slice(),
        [PersistedStructuredRolloutEvent::SpawnAgent {
            event_id,
            agent_id: event_agent_id,
            name: Some(name),
            child_session_id: event_child_session_id,
            status,
            summary: Some(summary),
            ..
        }] if event_id.starts_with("spawn-")
            && event_agent_id == agent_id
            && name == "Review Worker"
            && event_child_session_id == child_session_id
            && status == result_status
            && summary == result_summary
    ));
}

#[tokio::test]
async fn spawn_agent_writes_parent_scoped_sidechain_transcript() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let tool = AgentTool {
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
        workspace: Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir.clone())),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
    };

    let mut progress = |_| {};
    let result = tool
        .call_with_context_events(
            json!({
                "name": "General Worker",
                "instruction": "summarize this task"
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("spawn_agent result");

    let agent_id = result["agent_id"].as_str().expect("agent_id");
    let child_session_id = result["session_id"].as_str().expect("session_id");
    assert!(agent_id.starts_with("general-worker-"));
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
            name: Some(name),
            child_session_id: event_child_session_id,
            status,
            ..
        }] if event_agent_id == agent_id
            && name == "General Worker"
            && event_child_session_id == child_session_id
            && status == "done"
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
    assert_eq!(child.metadata.origin_kind, "subagent");
    assert_eq!(
        child.metadata.forked_from_thread_id.as_deref(),
        Some("parent-session")
    );
    assert!(!child.history.is_empty());
}

#[tokio::test]
async fn spawn_agent_definition_affects_prompt_tools_max_turns_and_plan_mode() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara-runtime");
    let agents_dir = root.join(".rara").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("agents dir");
    std::fs::write(root.join("fixture.txt"), "fixture contents").expect("fixture file");
    std::fs::write(
        agents_dir.join("code-reviewer.md"),
        r#"---
name: code-reviewer
description: Reviews code changes
tools: [Read, Grep]
disallowedTools: [Bash]
maxTurns: 1
planModeRequired: true
---

Custom reviewer prompt from workspace definition.
"#,
    )
    .expect("agent definition");

    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let tool = AgentTool {
        backend: Arc::new(DefinitionRegressionBackend {
            calls: calls.clone(),
            observed: observed.clone(),
        }),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
    };

    let mut progress = |_| {};
    let result = tool
        .call_with_context_events(
            json!({
                "name": "Code Reviewer",
                "instruction": "Inspect fixture.txt and report findings."
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("spawn_agent result");

    assert_eq!(result["status"], "done");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "maxTurns: 1 should stop after the first tool-calling turn"
    );
    let observed = observed.lock().expect("observed requests lock");
    let request = observed.first().expect("captured model request");
    let system_prompt = request
        .messages
        .iter()
        .find(|message| message.role == "system")
        .map(message_text)
        .expect("system prompt");
    assert!(!system_prompt.contains("Planning mode is active."));
    assert!(request.messages.iter().any(|message| {
        message.role == "user"
            && message.content.as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block.get("kind").and_then(serde_json::Value::as_str) == Some("execution_mode")
                        && block
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|text| text.contains("Planning mode is active."))
                })
            })
    }));
    assert!(system_prompt.contains("You are a custom workspace sub-agent."));
    assert!(system_prompt.contains(
        "Inspect repository or web evidence only through the read-only tools exposed to you."
    ));
    assert!(!system_prompt.contains("You do not have repository"));
    assert!(!system_prompt.contains("answer only from the provided instruction/context"));
    assert!(system_prompt.contains("Custom reviewer prompt from workspace definition."));
    assert!(
        system_prompt
            .find("## Plugin Capability Policy")
            .expect("capability policy")
            > system_prompt
                .find("Custom reviewer prompt from workspace definition.")
                .expect("custom prompt")
    );

    let mut tool_names = request.tool_names.clone();
    tool_names.sort();
    assert_eq!(
        tool_names,
        vec!["grep".to_string(), "read_file".to_string()]
    );
}

#[tokio::test]
async fn spawn_agent_definition_routes_provider_model_override() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara-runtime");
    let agents_dir = root.join(".rara").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("agents dir");
    std::fs::write(
        agents_dir.join("deep-reviewer.md"),
        r#"---
name: deep-reviewer
description: Reviews with a provider-specific model
provider: deepseek
model: deepseek-reasoner
maxTurns: 1
---

Use the configured DeepSeek backend.
"#,
    )
    .expect("agent definition");

    let resolver = Arc::new(RecordingBackendResolver::default());
    let targets = resolver.targets.clone();
    let tool = AgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        backend_resolver: resolver,
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
    };

    let result = tool
        .call(json!({
            "name": "Deep Reviewer",
            "instruction": "summarize routing"
        }))
        .await
        .expect("spawn_agent result");

    assert_eq!(result["provider"], "deepseek");
    assert_eq!(result["model"], "deepseek-reasoner");
    assert_eq!(
        targets.lock().expect("targets").as_slice(),
        [Some(SubagentProviderTarget {
            provider: Some("deepseek".to_string()),
            model: Some("deepseek-reasoner".to_string()),
        })]
    );
}

#[tokio::test]
async fn spawn_agent_invocation_model_keeps_profile_provider() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara-runtime");
    let agents_dir = root.join(".rara").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("agents dir");
    std::fs::write(
        agents_dir.join("reviewer.md"),
        r#"---
name: reviewer
description: Reviews with a profile model
provider: deepseek
model: deepseek-reasoner
maxTurns: 1
---

Review the assigned task.
"#,
    )
    .expect("agent definition");

    let resolver = Arc::new(RecordingBackendResolver::default());
    let targets = resolver.targets.clone();
    let tool = AgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        backend_resolver: resolver,
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
    };

    let result = tool
        .call(json!({
            "name": "reviewer",
            "instruction": "summarize routing",
            "model": "deepseek-chat"
        }))
        .await
        .expect("spawn_agent result");

    assert_eq!(result["provider"], "deepseek");
    assert_eq!(result["model"], "deepseek-chat");
    assert_eq!(
        targets.lock().expect("targets").as_slice(),
        [Some(SubagentProviderTarget {
            provider: Some("deepseek".to_string()),
            model: Some("deepseek-chat".to_string()),
        })]
    );
}

#[tokio::test]
async fn explore_agent_routes_per_invocation_model_override() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara-runtime");
    std::fs::create_dir_all(&root).expect("workspace");
    let resolver = Arc::new(RecordingBackendResolver::default());
    let targets = resolver.targets.clone();
    let tool = ExploreAgentTool {
        backend: Arc::new(CountingBackend {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        backend_resolver: resolver,
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir)),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
    };

    let result = tool
        .call(json!({
            "instruction": "inspect routing",
            "model": "openrouter:qwen/qwen3-coder"
        }))
        .await
        .expect("explore_agent result");

    assert_eq!(result["provider"], "openrouter");
    assert_eq!(result["model"], "qwen/qwen3-coder");
    assert_eq!(
        targets.lock().expect("targets").as_slice(),
        [Some(SubagentProviderTarget {
            provider: Some("openrouter".to_string()),
            model: Some("qwen/qwen3-coder".to_string()),
        })]
    );
}

#[tokio::test]
async fn team_create_routes_per_task_provider_model_override() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(&root).expect("workspace");
    let resolver = Arc::new(RecordingBackendResolver::default());
    let targets = resolver.targets.clone();
    let tool = TeamCreateTool {
        backend: Arc::new(MockLlm),
        backend_resolver: resolver,
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

    let result = tool
        .call(json!({
            "tasks": [
                {
                    "name": "analysis",
                    "kind": "general",
                    "provider": "gemini",
                    "model": "gemini-2.5-pro",
                    "instruction": "summarize one"
                },
                {
                    "name": "routing",
                    "kind": "general",
                    "model": "openrouter:anthropic/claude-sonnet-4",
                    "instruction": "summarize two"
                }
            ]
        }))
        .await
        .expect("team_create result");

    let results = result["team_results"].as_array().expect("results");
    assert_eq!(results[0]["provider"], "gemini");
    assert_eq!(results[0]["model"], "gemini-2.5-pro");
    assert_eq!(results[1]["provider"], "openrouter");
    assert_eq!(results[1]["model"], "anthropic/claude-sonnet-4");
    assert_eq!(
        targets.lock().expect("targets").as_slice(),
        [
            Some(SubagentProviderTarget {
                provider: Some("gemini".to_string()),
                model: Some("gemini-2.5-pro".to_string()),
            }),
            Some(SubagentProviderTarget {
                provider: Some("openrouter".to_string()),
                model: Some("anthropic/claude-sonnet-4".to_string()),
            }),
        ]
    );
}

#[tokio::test]
async fn spawn_agent_definition_token_budget_stops_after_budget_exhaustion() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let rara_dir = temp.path().join(".rara-runtime");
    let agents_dir = root.join(".rara").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("agents dir");
    std::fs::write(root.join("fixture.txt"), "fixture contents").expect("fixture file");
    std::fs::write(
        agents_dir.join("budget-reviewer.md"),
        r#"---
name: budget-reviewer
description: Reviews with a token budget
tools: [Read]
tokenBudget: 10
---

Review with a small budget.
"#,
    )
    .expect("agent definition");

    let calls = Arc::new(AtomicUsize::new(0));
    let tool = AgentTool {
        backend: Arc::new(BudgetedToolBackend {
            calls: calls.clone(),
        }),
        backend_resolver: inherited_backend_resolver(),
        memory_handle: Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager: Arc::new(
            SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
        ),
        workspace: Arc::new(WorkspaceMemory::from_paths(root.clone(), rara_dir.clone())),
        prompt_config: PromptRuntimeConfig::default(),
        task_list_id: test_task_list_id(),
        background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        agent_definitions: test_agent_definition_cache(&root),
        skill_manager: Arc::new(std::sync::RwLock::new(SkillManager::new())),
    };

    let mut progress = |_| {};
    let result = tool
        .call_with_context_events(
            json!({
                "name": "Budget Reviewer",
                "instruction": "Inspect fixture.txt and report findings."
            }),
            ToolCallContext::default().with_session_id("parent-session"),
            &mut progress,
        )
        .await
        .expect("spawn_agent result");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(result["status"], "budget_limited");
    assert_eq!(result["token_budget"], 10);
    assert_eq!(result["token_budget_exhausted"], true);
    assert!(
        result["summary"]
            .as_str()
            .expect("summary")
            .contains("Token budget exhausted: 15 / 10 tokens")
    );

    let events =
        thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
            .expect("rollout events");
    assert!(matches!(
        events.as_slice(),
        [PersistedStructuredRolloutEvent::SpawnAgent {
            status,
            token_budget: Some(10),
            ..
        }] if status == "budget_limited"
    ));
}
