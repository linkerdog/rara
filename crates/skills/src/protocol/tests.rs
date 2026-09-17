use super::*;

fn registration(source: &str, name: &str, priority: i32) -> ProtocolSkillRegistration {
    ProtocolSkillRegistration {
        source_id: source.into(),
        name: name.into(),
        content: format!("# {name}\nCompact description.\n\nFull instructions from {source}."),
        precedence_hint: Some(priority),
    }
}

fn local_skill(name: &str) -> Skill {
    Skill {
        name: name.into(),
        title: None,
        description: "Local description".into(),
        path: PathBuf::from(format!(".agents/skills/{name}/SKILL.md")),
        scope: SkillScope::Workspace,
        content: "# Local\nLocal instructions".into(),
        disable_model_invocation: false,
    }
}

#[test]
fn protocol_resolution_respects_priority_original_order_and_local_authority() {
    let mut manager = SkillManager::new();
    manager
        .register_protocol_skill(registration("z-first", "review", 0))
        .unwrap();
    manager
        .register_protocol_skill(registration("a-second", "review", 0))
        .unwrap();
    assert_eq!(manager.winning_protocol_source("review"), Some("z-first"));
    manager
        .register_protocol_skill(registration("third", "review", 10))
        .unwrap();
    assert_eq!(manager.winning_protocol_source("review"), Some("third"));
    manager
        .register_protocol_skill(registration("z-first", "review", 10))
        .unwrap();
    assert_eq!(
        manager.winning_protocol_source("review"),
        Some("z-first"),
        "replacement must retain registration order"
    );
    assert_eq!(
        manager
            .override_chain("review")
            .iter()
            .map(|skill| skill.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["protocol:third/review", "protocol:a-second/review"]
    );
    manager
        .disable_protocol_skill("review", Some("z-first"))
        .unwrap();
    assert_eq!(manager.winning_protocol_source("review"), Some("third"));
    assert_eq!(
        manager
            .protocol_skill_statuses()
            .iter()
            .filter(|entry| entry.selected)
            .count(),
        1
    );

    manager
        .skills
        .insert("review".into(), local_skill("review"));
    manager
        .register_protocol_skill(registration("a-second", "review", i32::MAX))
        .unwrap();
    assert_eq!(
        manager.get_skill("review").unwrap().scope,
        SkillScope::Workspace
    );
    assert_eq!(manager.winning_protocol_source("review"), None);
    assert!(
        manager
            .protocol_skill_statuses()
            .iter()
            .all(|entry| !entry.selected)
    );
    assert!(manager.shadows_others("review"));
    assert_eq!(manager.list_overrides()["review"].len(), 2);
    manager.disable_protocol_skill("review", None).unwrap();
    assert_eq!(
        manager.get_skill("review").unwrap().scope,
        SkillScope::Workspace
    );
    assert_eq!(
        manager.disable_protocol_skill("review", Some("missing")),
        Err(ProtocolSkillError::NotFound)
    );
}

#[test]
fn invalid_replacements_leave_the_previous_body_and_order_intact() {
    let mut manager = SkillManager::new();
    manager
        .register_protocol_skill(registration("source", "review", 0))
        .unwrap();
    let original = manager.get_skill("review").unwrap().content.clone();
    let status = manager.protocol_skill_statuses();
    for content in [
        "".into(),
        " \n ".into(),
        "---\nname: review\n---".into(),
        "x".repeat(MAX_PROTOCOL_SKILL_BYTES + 1),
    ] {
        let mut replacement = registration("source", "review", 99);
        replacement.content = content;
        assert!(manager.register_protocol_skill(replacement).is_err());
        assert_eq!(manager.get_skill("review").unwrap().content, original);
        assert_eq!(manager.protocol_skill_statuses(), status);
    }
    for invalid in [
        "".into(),
        "two words".into(),
        "x".repeat(MAX_ID_BYTES + 1),
        "\u{2603}".into(),
    ] {
        let mut candidate = registration("source", "review", 0);
        candidate.source_id = invalid.clone();
        assert_eq!(
            manager.register_protocol_skill(candidate),
            Err(ProtocolSkillError::Invalid)
        );
        let mut candidate = registration("source", "review", 0);
        candidate.name = invalid;
        assert_eq!(
            manager.register_protocol_skill(candidate),
            Err(ProtocolSkillError::Invalid)
        );
    }
    assert_eq!(manager.protocol_skill_statuses(), status);
}

#[test]
fn count_and_byte_capacity_include_disabled_definitions() {
    let mut manager = SkillManager::new();
    for index in 0..MAX_PROTOCOL_SKILLS {
        manager
            .register_protocol_skill(registration("source", &format!("skill-{index}"), 0))
            .unwrap();
    }
    manager.disable_protocol_skill("skill-0", None).unwrap();
    assert_eq!(
        manager.register_protocol_skill(registration("source", "overflow", 0)),
        Err(ProtocolSkillError::Capacity)
    );
    manager
        .register_protocol_skill(registration("source", "skill-0", 1))
        .unwrap();
    assert!(manager.get_skill("skill-0").is_some());
    assert_eq!(manager.protocol_skill_statuses().len(), MAX_PROTOCOL_SKILLS);

    let mut manager = SkillManager::new();
    for index in 0..MAX_PROTOCOL_TOTAL_BYTES / MAX_PROTOCOL_SKILL_BYTES {
        let mut entry = registration("source", &format!("skill-{index}"), 0);
        entry.content = "x".repeat(MAX_PROTOCOL_SKILL_BYTES);
        manager.register_protocol_skill(entry).unwrap();
    }
    manager.disable_protocol_skill("skill-0", None).unwrap();
    assert_eq!(
        manager.register_protocol_skill(registration("source", "overflow", 0)),
        Err(ProtocolSkillError::Capacity)
    );
    manager
        .register_protocol_skill(registration("source", "skill-0", 0))
        .unwrap();
    manager
        .register_protocol_skill(registration("source", "now-fits", 0))
        .unwrap();
    let original = manager.get_skill("skill-0").unwrap().content.clone();
    let mut too_large = registration("source", "skill-0", 0);
    too_large.content = "x".repeat(MAX_PROTOCOL_SKILL_BYTES);
    assert_eq!(
        manager.register_protocol_skill(too_large),
        Err(ProtocolSkillError::Capacity)
    );
    assert_eq!(manager.get_skill("skill-0").unwrap().content, original);
}

#[test]
fn local_reload_preserves_protocol_bodies_and_recomputes_winners() {
    let mut manager = SkillManager::new();
    manager
        .register_protocol_skill(registration("source", "review", 10))
        .unwrap();
    manager
        .register_protocol_skill(registration("source", "disabled", 0))
        .unwrap();
    manager.disable_protocol_skill("disabled", None).unwrap();
    let mut fresh = SkillManager::new();
    fresh.skills.insert("review".into(), local_skill("review"));
    fresh.load_warnings.push("local discovery warning".into());
    manager.replace_local_catalogue(fresh);
    assert_eq!(
        manager.get_skill("review").unwrap().scope,
        SkillScope::Workspace
    );
    assert_eq!(manager.load_warnings, ["local discovery warning"]);
    assert_eq!(manager.override_chain("review").len(), 1);
    assert!(manager.get_skill("disabled").is_none());
    manager.replace_local_catalogue(SkillManager::new());
    assert_eq!(
        manager.get_skill("review").unwrap().scope,
        SkillScope::Protocol
    );
    assert_eq!(manager.active_scopes(), ["protocol"]);
    assert!(manager.get_skill("disabled").is_none());
    assert_eq!(manager.protocol_skill_statuses().len(), 2);
}

#[test]
fn listing_is_sorted_and_body_disclosure_is_explicit() {
    let mut manager = SkillManager::new();
    manager
        .register_protocol_skill(registration("source", "z-skill", 0))
        .unwrap();
    let mut entry = registration("source", "a-skill", 0);
    entry.content = "---\nname: ignored-native-metadata\n---\n# Title\nShort description.\n\nOnly invocation may disclose this body marker.".into();
    manager.register_protocol_skill(entry).unwrap();
    let summaries = manager.list_summaries();
    assert_eq!(
        summaries
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        ["a-skill", "z-skill"]
    );
    assert_eq!(summaries[0].description, "Short description.");
    assert_eq!(summaries[0].source_id.as_deref(), Some("source"));
    let body = manager.get_skill("a-skill").unwrap().instructions();
    assert!(body.contains("Only invocation may disclose this body marker."));
    assert!(!body.contains("ignored-native-metadata"));
}
