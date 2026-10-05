use super::*;

fn registration(id: &str, content: String) -> PromptSourceRegistration {
    PromptSourceRegistration {
        source_id: id.into(),
        scope: SourceScope::Protocol,
        layer: SourceLayer::User,
        budget_hint_tokens: None,
        lifetime: PromptSourceLifetime::Session,
        content,
    }
}

async fn register(
    registry: &PromptSourceRegistry,
    registration: PromptSourceRegistration,
) -> Result<(), PromptSourceError> {
    registry
        .handle_control_with_provenance(
            &PromptSourceControlRequest::Register(registration),
            RuntimeProvenance::runtime(Some("session-1".into())),
        )
        .await
}

#[tokio::test]
async fn invalid_or_unsupported_registration_has_no_effect() {
    let bus = Arc::new(RuntimeEventBus::new(16));
    let mut events = bus.subscribe_control();
    let registry = PromptSourceRegistry::new(bus);
    let valid = registration("source-1", "context".into());
    let mut cases = Vec::new();
    let mut empty_turns = valid.clone();
    empty_turns.lifetime = PromptSourceLifetime::Turns(0);
    cases.push((empty_turns, PromptSourceError::Invalid));
    let mut persistent = valid.clone();
    persistent.lifetime = PromptSourceLifetime::Persistent;
    cases.push((persistent, PromptSourceError::Unsupported));
    let mut system = valid.clone();
    system.layer = SourceLayer::System;
    cases.push((system, PromptSourceError::Unsupported));
    let mut global = valid.clone();
    global.scope = SourceScope::Home;
    cases.push((global, PromptSourceError::Unsupported));
    let mut invalid_id = valid.clone();
    invalid_id.source_id = "injected\nlabel".into();
    cases.push((invalid_id, PromptSourceError::Invalid));
    let mut oversized = valid.clone();
    oversized.content = "x".repeat(MAX_PROMPT_SOURCE_BYTES + 1);
    cases.push((oversized, PromptSourceError::Capacity));
    for (registration, expected) in cases {
        assert_eq!(register(&registry, registration).await, Err(expected));
    }
    assert_eq!(
        registry
            .handle_control_with_provenance(
                &PromptSourceControlRequest::Register(valid),
                RuntimeProvenance::protocol(
                    crate::runtime_control::RuntimeControllerKind::AppServer,
                    "invalid\nadapter",
                    Some("session-1".into()),
                    None
                ),
            )
            .await,
        Err(PromptSourceError::Invalid)
    );
    assert!(registry.list_prompt_sources_for_query().await.is_empty());
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn registration_count_allows_replacement_without_forgetting_other_sources() {
    let registry = PromptSourceRegistry::new(Arc::new(RuntimeEventBus::new(4)));
    for index in 0..MAX_PROMPT_SOURCES {
        register(
            &registry,
            registration(&format!("source-{index}"), "context".into()),
        )
        .await
        .expect("source");
    }
    register(&registry, registration("source-0", "replacement".into()))
        .await
        .expect("replacement at capacity");
    assert_eq!(
        register(&registry, registration("overflow", "context".into())).await,
        Err(PromptSourceError::Capacity)
    );
    let sources = registry.list_prompt_sources_for_query().await;
    assert_eq!(sources.len(), MAX_PROMPT_SOURCES);
    assert_eq!(
        sources
            .iter()
            .find(|source| source.label.ends_with("source-0"))
            .expect("replaced source")
            .content,
        "replacement"
    );
}

#[tokio::test]
async fn aggregate_budget_rejects_growth_atomically_and_reclaims_replaced_bytes() {
    let registry = PromptSourceRegistry::new(Arc::new(RuntimeEventBus::new(4)));
    for index in 0..3 {
        register(
            &registry,
            registration(
                &format!("large-{index}"),
                "x".repeat(MAX_PROMPT_SOURCE_BYTES),
            ),
        )
        .await
        .expect("large source");
    }
    register(
        &registry,
        registration("tail", "x".repeat(MAX_PROMPT_SOURCE_BYTES - 1)),
    )
    .await
    .expect("tail");
    register(&registry, registration("tiny", "x".into()))
        .await
        .expect("exact aggregate limit");
    assert_eq!(
        register(&registry, registration("tiny", "xx".into())).await,
        Err(PromptSourceError::Capacity)
    );
    let sources = registry.list_prompt_sources_for_query().await;
    assert_eq!(
        sources
            .iter()
            .map(|source| source.content.len())
            .sum::<usize>(),
        MAX_PROMPT_CONTENT_BYTES
    );
    assert_eq!(
        sources
            .iter()
            .find(|source| source.label.ends_with("tiny"))
            .expect("unchanged source")
            .content,
        "x"
    );
    register(&registry, registration("large-0", "small".into()))
        .await
        .expect("shrink source");
    register(&registry, registration("tiny", "xx".into()))
        .await
        .expect("freed capacity");
}

#[tokio::test]
async fn lifecycle_events_preserve_source_provenance_through_query_and_expiry() {
    let bus = Arc::new(RuntimeEventBus::new(16));
    let mut events = bus.subscribe_control();
    let registry = PromptSourceRegistry::new(bus);
    let provenance = RuntimeProvenance::protocol(
        crate::runtime_control::RuntimeControllerKind::AppServer,
        "stdio-jsonl",
        Some("session-1".into()),
        Some("source-1".into()),
    );
    let mut source = registration("source-1", "context".into());
    source.lifetime = PromptSourceLifetime::Turns(1);
    registry
        .handle_control_with_provenance(
            &PromptSourceControlRequest::Register(source),
            provenance.clone(),
        )
        .await
        .expect("register");
    registry
        .handle_control_with_provenance(
            &PromptSourceControlRequest::QuerySources,
            RuntimeProvenance::runtime(None),
        )
        .await
        .expect("query");
    assert_eq!(registry.list_prompt_sources_for_query().await.len(), 1);
    assert!(registry.list_prompt_sources_for_query().await.is_empty());
    let mut observed = Vec::new();
    while let Ok(event) = events.try_recv() {
        assert_eq!(event.provenance, provenance);
        observed.push(event.event);
    }
    assert_eq!(
        observed,
        vec![
            RuntimeEvent::PromptSource(PromptSourceEvent::Registered {
                source_id: "source-1".into()
            }),
            RuntimeEvent::PromptSource(PromptSourceEvent::Registered {
                source_id: "source-1".into()
            }),
            RuntimeEvent::PromptSource(PromptSourceEvent::Injected {
                source_id: "source-1".into()
            }),
            RuntimeEvent::PromptSource(PromptSourceEvent::Dropped {
                source_id: "source-1".into(),
                reason: "turn limit expired".into()
            }),
        ]
    );
}
