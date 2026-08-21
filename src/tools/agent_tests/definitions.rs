use super::*;

#[test]
fn tool_manager_retain_filters_tools_by_name() {
    let mut manager = build_read_only_tool_manager(test_task_store(), DEFAULT_TASK_LIST_ID);
    assert!(manager.get_tool("grep").is_some());
    assert!(manager.get_tool("glob").is_some());
    manager.retain(|name| name == "grep");
    assert!(manager.get_tool("grep").is_some());
    assert!(manager.get_tool("glob").is_none());
}

#[test]
fn filtered_tool_manager_respects_tools_whitelist() {
    let definition = AgentDefinition {
        token_budget: None,
        name: "custom".into(),
        description: "custom".into(),
        tools: vec!["Grep".into(), "Read".into()],
        disallowed_tools: vec![],
        plugin_skills: vec![],
        model: None,
        provider: None,
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    };
    let manager = build_filtered_tool_manager(
        SubAgentKind::Explore,
        &definition,
        test_task_root(),
        DEFAULT_TASK_LIST_ID,
    )
    .expect("filtered manager");
    assert!(manager.get_tool("grep").is_some());
    assert!(manager.get_tool("read_file").is_some());
    assert!(manager.get_tool("glob").is_none());
    assert!(manager.get_tool("list_files").is_none());
}

#[test]
fn filtered_tool_manager_maps_task_tool_aliases() {
    let definition = AgentDefinition {
        token_budget: None,
        name: "custom".into(),
        description: "custom".into(),
        tools: vec!["TaskList".into(), "TaskGet".into()],
        disallowed_tools: vec![],
        plugin_skills: vec![],
        model: None,
        provider: None,
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    };
    let manager = build_filtered_tool_manager(
        SubAgentKind::Explore,
        &definition,
        test_task_root(),
        DEFAULT_TASK_LIST_ID,
    )
    .expect("filtered manager");
    assert!(manager.get_tool("task_list").is_some());
    assert!(manager.get_tool("task_get").is_some());
    assert!(manager.get_tool("read_file").is_none());
}

#[test]
fn filtered_tool_manager_respects_disallowed_tools_blacklist() {
    let definition = AgentDefinition {
        token_budget: None,
        name: "custom".into(),
        description: "custom".into(),
        tools: vec![],
        disallowed_tools: vec!["Grep".into(), "Glob".into()],
        plugin_skills: vec![],
        model: None,
        provider: None,
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    };
    let manager = build_filtered_tool_manager(
        SubAgentKind::Explore,
        &definition,
        test_task_root(),
        DEFAULT_TASK_LIST_ID,
    )
    .expect("filtered manager");
    assert!(manager.get_tool("read_file").is_some());
    assert!(manager.get_tool("list_files").is_some());
    assert!(manager.get_tool("grep").is_none());
    assert!(manager.get_tool("glob").is_none());
}

#[test]
fn filtered_tool_manager_disallowed_takes_precedence_over_tools() {
    let definition = AgentDefinition {
        token_budget: None,
        name: "custom".into(),
        description: "custom".into(),
        tools: vec!["Grep".into(), "Read".into()],
        disallowed_tools: vec!["Grep".into()],
        plugin_skills: vec![],
        model: None,
        provider: None,
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    };
    let manager = build_filtered_tool_manager(
        SubAgentKind::Explore,
        &definition,
        test_task_root(),
        DEFAULT_TASK_LIST_ID,
    )
    .expect("filtered manager");
    assert!(
        manager.get_tool("read_file").is_some(),
        "Read should be allowed"
    );
    assert!(
        manager.get_tool("grep").is_none(),
        "Grep should be blocked by disallowed_tools"
    );
}

#[test]
fn filtered_tool_manager_permission_mode_plan_forces_read_only_tools() {
    let definition = AgentDefinition {
        token_budget: None,
        name: "custom".into(),
        description: "custom".into(),
        tools: vec![],
        disallowed_tools: vec![],
        plugin_skills: vec![],
        model: None,
        provider: None,
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: Some("plan".into()),
        hidden: false,
        system_prompt: String::new(),
    };

    let manager = build_filtered_tool_manager(
        SubAgentKind::General,
        &definition,
        test_task_root(),
        DEFAULT_TASK_LIST_ID,
    )
    .expect("filtered manager");

    assert!(manager.get_tool("read_file").is_some());
    assert!(manager.get_tool("task_get").is_some());
    assert!(manager.get_tool("task_create").is_none());
    assert!(manager.get_tool("task_update").is_none());
}

#[test]
fn filtered_tool_manager_rejects_unknown_permission_mode() {
    let definition = AgentDefinition {
        token_budget: None,
        name: "custom".into(),
        description: "custom".into(),
        tools: vec![],
        disallowed_tools: vec![],
        plugin_skills: vec![],
        model: None,
        provider: None,
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: Some("surprise".into()),
        hidden: false,
        system_prompt: String::new(),
    };

    let err = match build_filtered_tool_manager(
        SubAgentKind::General,
        &definition,
        test_task_root(),
        DEFAULT_TASK_LIST_ID,
    ) {
        Ok(_) => panic!("invalid permission mode should fail"),
        Err(err) => err,
    };

    assert!(
        matches!(err, ToolError::InvalidInput(message) if message.contains("permissionMode")
            && message.contains("readOnly")
            && message.contains("fullAccess"))
    );
}

#[test]
fn agent_permission_mode_maps_runtime_permissions() {
    assert_eq!(
        parse_agent_permission_mode("acceptEdits")
            .expect("acceptEdits")
            .bash_approval_mode(false),
        crate::agent::BashApprovalMode::Suggestion
    );
    assert!(
        !parse_agent_permission_mode("acceptEdits")
            .expect("acceptEdits")
            .full_access_mode(false)
    );

    let plan = parse_agent_permission_mode("plan").expect("plan");
    assert!(plan.requires_plan_mode());
    assert_eq!(
        plan.bash_approval_mode(true),
        crate::agent::BashApprovalMode::Suggestion
    );

    let bypass = parse_agent_permission_mode("bypassPermissions").expect("bypass");
    assert_eq!(
        bypass.bash_approval_mode(false),
        crate::agent::BashApprovalMode::Always
    );
    assert!(bypass.full_access_mode(false));
    assert!(!bypass.full_access_mode(true));
}

#[test]
fn agent_permission_mode_accepts_case_insensitive_aliases() {
    assert!(
        parse_agent_permission_mode("Plan")
            .expect("Plan")
            .requires_plan_mode()
    );
    assert!(
        parse_agent_permission_mode("BYPASSPERMISSIONS")
            .expect("BYPASSPERMISSIONS")
            .full_access_mode(false)
    );
    assert_eq!(
        parse_agent_permission_mode("acceptedits")
            .expect("acceptedits")
            .bash_approval_mode(false),
        crate::agent::BashApprovalMode::Suggestion
    );
}

#[test]
fn resolve_kind_definition_plan_sets_plan_mode_required() {
    let def = resolve_kind_definition(SubAgentKind::Plan);
    assert!(def.plan_mode_required);
    assert_eq!(def.name, "plan");
}

#[test]
fn resolve_kind_definition_explore_no_plan_mode() {
    let def = resolve_kind_definition(SubAgentKind::Explore);
    assert!(!def.plan_mode_required);
    assert_eq!(def.name, "explore");
}

#[test]
fn resolve_spawn_agent_definition_resolves_builtin() {
    let temp = tempdir().expect("tempdir");
    let cache = test_agent_definition_cache(temp.path());
    let def = resolve_spawn_agent_definition(&cache, "explore");
    assert_eq!(def.name, "explore");
}

#[test]
fn resolve_spawn_agent_definition_resolves_builtin_specialists() {
    let temp = tempdir().expect("tempdir");
    let cache = test_agent_definition_cache(temp.path());

    for (name, prompt_fragment) in [
        ("code-reviewer", "independent code reviewer"),
        ("architect", "software architecture specialist"),
    ] {
        let definition = resolve_spawn_agent_definition(&cache, name);

        assert_eq!(definition.name, name);
        assert_eq!(definition.tools, vec!["Read", "Glob", "Grep"]);
        assert_eq!(definition.max_turns, 50);
        assert!(!definition.plan_mode_required);
        assert!(definition.system_prompt.contains(prompt_fragment));
    }

    let researcher = resolve_spawn_agent_definition(&cache, "researcher");
    assert_eq!(
        researcher.tools,
        vec!["Read", "Glob", "Grep", "WebSearch", "WebFetch"]
    );
    assert_eq!(researcher.max_turns, 50);
    assert!(!researcher.plan_mode_required);
    assert!(
        researcher
            .system_prompt
            .contains("source URL or repository file path")
    );
    assert!(researcher.system_prompt.contains("Treat search results as"));
}

#[test]
fn builtin_specialist_tool_managers_are_read_only() {
    let temp = tempdir().expect("tempdir");
    let cache = test_agent_definition_cache(temp.path());

    for name in ["code-reviewer", "architect", "researcher"] {
        let definition = resolve_spawn_agent_definition(&cache, name);
        let manager = build_filtered_tool_manager(
            SubAgentKind::General,
            &definition,
            test_task_root(),
            DEFAULT_TASK_LIST_ID,
        )
        .expect("built-in specialist tool manager");

        assert!(manager.get_tool("read_file").is_some());
        assert!(manager.get_tool("glob").is_some());
        assert!(manager.get_tool("grep").is_some());
        assert_eq!(
            manager.get_tool("web_search").is_some(),
            name == "researcher"
        );
        assert_eq!(
            manager.get_tool("web_fetch").is_some(),
            name == "researcher"
        );
        assert!(manager.get_tool("task_create").is_none());
        assert!(manager.get_tool("task_update").is_none());
        assert!(manager.get_tool("bash").is_none());
        assert!(manager.get_tool("write_file").is_none());
        assert!(manager.get_tool("apply_patch").is_none());
        assert!(manager.get_tool("spawn_agent").is_none());
    }
}

#[test]
fn researcher_role_prompt_describes_read_only_web_evidence_access() {
    let temp = tempdir().expect("tempdir");
    let cache = test_agent_definition_cache(temp.path());
    let researcher = resolve_spawn_agent_definition(&cache, "researcher");

    let prompt = subagent_role_prompt(SubAgentKind::General, Some(&researcher));

    assert!(prompt.contains("repository or web evidence"));
    assert!(prompt.contains("interactive browser automation"));
    assert!(!prompt.contains("You do not have shell, editing, patching, browser,"));
}

#[test]
fn resolve_spawn_agent_definition_falls_back_for_unknown() {
    let temp = tempdir().expect("tempdir");
    let cache = test_agent_definition_cache(temp.path());
    let def = resolve_spawn_agent_definition(&cache, "unknown-agent");
    assert_eq!(def.name, "unknown-agent");
    assert!(!def.plan_mode_required);
}

#[test]
fn resolve_spawn_agent_definition_loads_workspace_agent() {
    let temp = tempdir().expect("tempdir");
    let agents_dir = temp.path().join(".rara").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("agents dir");
    std::fs::write(
        agents_dir.join("code-reviewer.md"),
        r#"---
name: code-reviewer
description: Reviews code changes
tools: [Read, Grep]
disallowedTools: [Bash]
maxTurns: 7
planModeRequired: true
---

Review the assigned change and report concrete findings.
"#,
    )
    .expect("agent definition");

    let cache = test_agent_definition_cache(temp.path());
    let def = resolve_spawn_agent_definition(&cache, "code-reviewer");

    assert_eq!(def.name, "code-reviewer");
    assert_eq!(def.description, "Reviews code changes");
    assert_eq!(def.tools, vec!["Read", "Grep"]);
    assert_eq!(def.disallowed_tools, vec!["Bash"]);
    assert_eq!(def.max_turns, 7);
    assert!(def.plan_mode_required);
    assert!(def.system_prompt.contains("Review the assigned change"));
}

#[test]
fn rara_agent_definition_overrides_legacy_claude_definition() {
    let temp = tempdir().expect("tempdir");
    let claude_agents_dir = temp.path().join(".claude").join("agents");
    std::fs::create_dir_all(&claude_agents_dir).expect("claude agents dir");
    std::fs::write(
        claude_agents_dir.join("helper.md"),
        r#"---
name: helper
description: Legacy helper
tools: [Read]
---

Legacy prompt.
"#,
    )
    .expect("legacy agent definition");

    let rara_agents_dir = temp.path().join(".rara").join("agents");
    std::fs::create_dir_all(&rara_agents_dir).expect("rara agents dir");
    std::fs::write(
        rara_agents_dir.join("helper.md"),
        r#"---
name: helper
description: RARA helper
tools: [Read, Grep]
---

RARA prompt.
"#,
    )
    .expect("rara agent definition");

    let cache = test_agent_definition_cache(temp.path());
    let def = resolve_spawn_agent_definition(&cache, "helper");

    assert_eq!(def.description, "RARA helper");
    assert_eq!(def.tools, vec!["Read", "Grep"]);
    assert_eq!(def.system_prompt, "RARA prompt.");
}

#[test]
fn agent_definition_uses_filename_when_frontmatter_omits_name() {
    let temp = tempdir().expect("tempdir");
    let agents_dir = temp.path().join(".rara").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("agents dir");
    std::fs::write(
        agents_dir.join("reviewer.md"),
        r#"---
tools: [Read]
---

Review the change.
"#,
    )
    .expect("agent definition");

    let cache = test_agent_definition_cache(temp.path());
    let def = resolve_spawn_agent_definition(&cache, "reviewer");

    assert_eq!(def.name, "reviewer");
    assert_eq!(def.description, "");
    assert_eq!(def.tools, vec!["Read"]);
    assert_eq!(def.system_prompt, "Review the change.");
}

#[test]
fn agent_definition_accepts_empty_frontmatter() {
    let temp = tempdir().expect("tempdir");
    let agents_dir = temp.path().join(".rara").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("agents dir");
    std::fs::write(
        agents_dir.join("helper.md"),
        r#"---
---

Help with the task.
"#,
    )
    .expect("agent definition");

    let cache = test_agent_definition_cache(temp.path());
    let def = resolve_spawn_agent_definition(&cache, "helper");

    assert_eq!(def.name, "helper");
    assert_eq!(def.description, "");
    assert!(def.tools.is_empty());
    assert_eq!(def.system_prompt, "Help with the task.");
}

#[test]
fn agent_definition_cache_refreshes_on_new_runtime_cache() {
    let temp = tempdir().expect("tempdir");
    let agents_dir = temp.path().join(".rara").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("agents dir");
    std::fs::write(
        agents_dir.join("helper.md"),
        r#"---
name: helper
description: Initial helper
---

Initial prompt.
"#,
    )
    .expect("agent definition");
    let cache = test_agent_definition_cache(temp.path());
    let first = resolve_spawn_agent_definition(&cache, "helper");
    assert_eq!(first.description, "Initial helper");

    std::fs::write(
        agents_dir.join("helper.md"),
        r#"---
name: helper
description: Reloaded helper
---

Reloaded prompt.
"#,
    )
    .expect("updated agent definition");

    let stale = resolve_spawn_agent_definition(&cache, "helper");
    assert_eq!(stale.description, "Initial helper");

    let reloaded_cache = test_agent_definition_cache(temp.path());
    let reloaded = resolve_spawn_agent_definition(&reloaded_cache, "helper");
    assert_eq!(reloaded.description, "Reloaded helper");
    assert_eq!(reloaded.system_prompt, "Reloaded prompt.");
}

#[test]
fn agent_home_dir_falls_back_to_userprofile() {
    let home = home_dir_from_vars(None, Some(std::ffi::OsString::from("C:\\Users\\rara")))
        .expect("home fallback");

    assert_eq!(home, std::path::PathBuf::from("C:\\Users\\rara"));
}

#[test]
fn spawn_agent_definition_lookup_uses_normalized_label() {
    let temp = tempdir().expect("tempdir");
    let agents_dir = temp.path().join(".rara").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("agents dir");
    std::fs::write(
        agents_dir.join("code-reviewer.md"),
        r#"---
name: code-reviewer
description: Reviews code changes
tools: [Read]
---

Review the assigned change.
"#,
    )
    .expect("agent definition");

    let label = validate_agent_id_label("Code Reviewer").expect("label");
    let cache = test_agent_definition_cache(temp.path());
    let def = resolve_spawn_agent_definition(&cache, &label);

    assert_eq!(label, "code-reviewer");
    assert_eq!(def.name, "code-reviewer");
    assert_eq!(def.tools, vec!["Read"]);
}

#[test]
fn explore_agent_definition_has_default_max_turns_50() {
    let def = resolve_kind_definition(SubAgentKind::Explore);
    assert_eq!(def.max_turns, 50);
}

#[test]
fn plan_agent_definition_has_default_max_turns_30() {
    let def = resolve_kind_definition(SubAgentKind::Plan);
    assert_eq!(def.max_turns, 30);
}

#[test]
fn general_agent_definition_has_unlimited_max_turns() {
    let def = resolve_kind_definition(SubAgentKind::General);
    assert_eq!(def.max_turns, 0);
}
