// Native runtime fixtures use explicit workspaces and a scripted provider.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::time::{Duration, timeout};

use super::*;
use crate::runtime_control::{
    RuntimeControlEvent, RuntimeControllerKind, SkillEvent, SkillSourceControlRequest,
};
use crate::{
    AgentOutputMode, ContentBlock, LlmBackend, LlmResponse, Message, RaraConfig, RuntimeEvent,
    RuntimeProvenance, Tool, ToolError, ToolManager,
};

const TIMEOUT: Duration = Duration::from_secs(5);
const SKILL_NAME: &str = "protocol-review";
const BODY: &str = "Inspect the uniquely marked private procedure body.";
const DESCRIPTION: &str = "Review protocol behavior.";
const CLEARED: &str = "No skills are currently available.";

#[derive(Default)]
struct Backend {
    responses: Mutex<VecDeque<LlmResponse>>,
    requests: Mutex<Vec<Vec<Message>>>,
    gate: Option<Notify>,
    started: Notify,
}

impl Backend {
    fn push(&self, content: Vec<ContentBlock>) {
        let uses_tool = content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { .. }));
        self.responses
            .lock()
            .expect("responses")
            .push_back(LlmResponse {
                content,
                stop_reason: Some(if uses_tool { "tool_use" } else { "end_turn" }.into()),
                usage: None,
            });
    }

    fn done(&self) {
        self.push(vec![ContentBlock::Text {
            text: "done".into(),
        }]);
    }

    fn tool(&self, action: &str) {
        self.push(vec![ContentBlock::ToolUse {
            id: format!("skill-{action}"),
            name: "skill".into(),
            input: json!({"action": action, "skill_name": SKILL_NAME}),
        }]);
        self.done();
    }
}

#[async_trait]
impl LlmBackend for Backend {
    async fn ask(&self, messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
        self.requests
            .lock()
            .expect("requests")
            .push(messages.to_vec());
        self.started.notify_one();
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        self.responses
            .lock()
            .expect("responses")
            .pop_front()
            .ok_or_else(|| anyhow!("unexpected provider call"))
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Err(anyhow!("skill fixture must not summarize"))
    }
}

fn builder(root: &std::path::Path, id: &str, backend: Arc<Backend>) -> RuntimeSessionBuilder {
    RuntimeSessionBuilder::new(RaraConfig::default(), root)
        .with_backend(backend)
        .with_session_id(id)
        .with_state_root(root.join(format!("state-{id}")))
        .without_extension_discovery()
        .without_memory_facilities()
        .without_transcript_persistence()
}

fn registration() -> SkillSourceControlRequest {
    SkillSourceControlRequest::RegisterSkill {
        source_id: "team-skill".into(),
        name: SKILL_NAME.into(),
        content: format!("---\ndescription: {DESCRIPTION}\n---\n{DESCRIPTION}\n\n{BODY}"),
        precedence_hint: Some(7),
    }
}

fn origin(session: &RuntimeSession) -> RuntimeProvenance {
    RuntimeProvenance::protocol(
        RuntimeControllerKind::AppServer,
        "stdio-jsonl",
        Some(session.id().to_string()),
        Some("team-skill".into()),
    )
}

async fn query(session: &RuntimeSession, prompt: &str) -> RuntimeTurnId {
    let turn = session
        .submit(prompt, AgentOutputMode::Silent)
        .await
        .expect("admit turn");
    let id = turn.id().clone();
    timeout(TIMEOUT, turn.wait())
        .await
        .expect("completion timeout")
        .expect("query outcome");
    id
}

async fn events(session: &RuntimeSession) -> Vec<RuntimeControlEvent> {
    let last_sequence = session.snapshot().last_sequence;
    let mut stream = session.subscribe_after(0).expect("replay window");
    let mut events = Vec::new();
    while stream.cursor() < last_sequence {
        events.push(
            timeout(TIMEOUT, stream.recv())
                .await
                .expect("event timeout")
                .expect("event"),
        );
    }
    events
}

#[tokio::test]
async fn registration_reaches_native_invocation_with_provenance_and_stable_context() {
    let root = tempfile::tempdir().expect("workspace");
    let backend = Arc::new(Backend::default());
    let session = builder(root.path(), "skills-a", backend.clone())
        .build()
        .await
        .expect("session");
    let backend_b = Arc::new(Backend::default());
    let session_b = builder(root.path(), "skills-b", backend_b.clone())
        .build()
        .await
        .expect("session B");
    backend.done();
    query(&session, "baseline").await;
    let stable_system = backend.requests.lock().expect("requests")[0][0]
        .content
        .clone();

    let mut observed = session.subscribe_control();
    assert!(matches!(
        session_b
            .apply_skill_source(registration(), origin(&session))
            .await,
        Err(RuntimeSessionError::InvalidSource)
    ));
    session
        .apply_skill_source(registration(), origin(&session))
        .await
        .expect("register");
    session
        .apply_skill_source(SkillSourceControlRequest::QuerySkills, origin(&session))
        .await
        .expect("query catalogue");
    let mut registered = Vec::new();
    loop {
        match observed.try_recv() {
            Ok(event) => registered.push(event),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(error) => panic!("registration observation failed: {error}"),
        }
    }
    assert!(!registered.iter().any(|event| matches!(
        event.event,
        RuntimeEvent::Skill(SkillEvent::Injected { .. })
    )));
    let catalogue = registered
        .iter()
        .rev()
        .find_map(|event| match &event.event {
            RuntimeEvent::Skill(SkillEvent::Catalogue { skills }) => Some(skills),
            _ => None,
        })
        .expect("catalogue");
    assert_eq!(catalogue.len(), 1);
    assert!(catalogue[0].enabled && catalogue[0].selected);
    assert!(
        !serde_json::to_string(&registered)
            .expect("serialized events")
            .contains(BODY)
    );

    backend.tool("list");
    query(&session, "discover").await;
    {
        let requests = backend.requests.lock().expect("requests");
        assert!(
            requests[1].iter().any(|message| message.role == "user"
                && message.content.to_string().contains(DESCRIPTION))
        );
        assert!(
            requests[2]
                .iter()
                .any(|message| message.content.to_string().contains("team-skill"))
        );
        assert!(
            requests
                .iter()
                .flatten()
                .all(|message| !message.content.to_string().contains(BODY))
        );
        assert!(
            requests
                .iter()
                .all(|messages| messages[0].content == stable_system)
        );
    }

    backend.tool("invoke");
    let invocation_turn = query(&session, "invoke").await;
    let invoked = events(&session).await;
    let injected = invoked
        .iter()
        .filter(|event| {
            matches!(
                event.event,
                RuntimeEvent::Skill(SkillEvent::Injected { .. })
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(injected.len(), 1);
    assert_eq!(
        injected[0].turn_id.as_deref(),
        Some(invocation_turn.as_str())
    );
    assert_eq!(injected[0].provenance, origin(&session));
    assert!(
        backend
            .requests
            .lock()
            .expect("requests")
            .last()
            .expect("tool result request")
            .iter()
            .any(|message| message.content.to_string().contains(BODY))
    );

    let original_history = session.transcript().await.expect("history");
    session
        .apply_skill_source(
            SkillSourceControlRequest::DisableSkill {
                name: SKILL_NAME.into(),
                source_id: Some("team-skill".into()),
            },
            origin(&session),
        )
        .await
        .expect("disable");
    backend.tool("invoke");
    query(&session, "cannot invoke disabled skill").await;
    backend.done();
    query(&session, "next query").await;
    let history = session.transcript().await.expect("history");
    assert_eq!(
        serde_json::to_value(&history[..original_history.len()]).expect("history"),
        serde_json::to_value(&original_history).expect("original history")
    );
    assert_eq!(
        history
            .iter()
            .filter(|message| message.content.to_string().contains(CLEARED))
            .count(),
        1
    );
    {
        let requests = backend.requests.lock().expect("requests");
        assert!(
            requests
                .iter()
                .all(|messages| messages[0].content == stable_system)
        );
        assert!(
            requests
                .last()
                .expect("latest request")
                .iter()
                .any(|message| message.content.to_string().contains("Skill not found"))
        );
    }
    let disabled = events(&session).await;
    assert_eq!(
        disabled
            .iter()
            .filter(|event| matches!(
                event.event,
                RuntimeEvent::Skill(SkillEvent::Injected { .. })
            ))
            .count(),
        1
    );
    assert!(disabled.iter().any(
        |event| matches!(&event.event, RuntimeEvent::Skill(SkillEvent::Catalogue { skills })
        if skills.len() == 1 && !skills[0].enabled && !skills[0].selected)
    ));

    backend_b.done();
    query(&session_b, "isolated query").await;
    assert!(
        backend_b
            .requests
            .lock()
            .expect("requests B")
            .iter()
            .flatten()
            .all(|message| !message.content.to_string().contains(SKILL_NAME))
    );
    session.shutdown().await.expect("shutdown A");
    session_b.shutdown().await.expect("shutdown B");
    assert!(matches!(
        session
            .apply_skill_source(registration(), origin(&session))
            .await,
        Err(RuntimeSessionError::Closed | RuntimeSessionError::ActorStopped)
    ));
}

#[tokio::test]
async fn active_turn_rejects_skill_mutation_and_disabled_discovery_cannot_reload() {
    let root = tempfile::tempdir().expect("workspace");
    let backend = Arc::new(Backend {
        gate: Some(Notify::new()),
        ..Default::default()
    });
    backend.done();
    let session = builder(root.path(), "busy-skills", backend.clone())
        .build()
        .await
        .expect("session");
    let turn = session
        .submit("hold", AgentOutputMode::Silent)
        .await
        .expect("admit");
    timeout(TIMEOUT, backend.started.notified())
        .await
        .expect("provider started");
    assert!(
        matches!(session.apply_skill_source(registration(), origin(&session)).await,
        Err(RuntimeSessionError::Busy { active_turn }) if &active_turn == turn.id())
    );
    backend.gate.as_ref().expect("gate").notify_one();
    timeout(TIMEOUT, turn.wait())
        .await
        .expect("completion timeout")
        .expect("complete");
    assert!(matches!(
        session
            .apply_skill_source(
                SkillSourceControlRequest::RegisterRoot {
                    source_id: "root".into(),
                    root: root.path().to_string_lossy().into_owned(),
                    precedence_hint: None,
                },
                origin(&session)
            )
            .await,
        Err(RuntimeSessionError::UnsupportedSource)
    ));
    session
        .apply_skill_source(registration(), origin(&session))
        .await
        .expect("register before replacement");
    // Replacing the backend retains the native catalogue and discovery policy.
    let reload_backend = Arc::new(Backend::default());
    reload_backend.tool("reload");
    session
        .replace_llm_backend(reload_backend.clone())
        .await
        .expect("replace backend");
    query(&session, "reload is disabled").await;
    assert!(
        reload_backend
            .requests
            .lock()
            .expect("requests")
            .last()
            .expect("tool result")
            .iter()
            .any(|message| message
                .content
                .to_string()
                .contains("skill reload is not available in this session"))
    );
    assert!(
        reload_backend.requests.lock().expect("requests")[0]
            .iter()
            .any(|message| message.content.to_string().contains(DESCRIPTION))
    );
    session.shutdown().await.expect("shutdown");
}

struct HostSkill;

#[async_trait]
impl Tool for HostSkill {
    fn name(&self) -> &str {
        "skill"
    }
    fn description(&self) -> &str {
        "Host-owned tool with no native catalogue"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn call(&self, _input: Value) -> std::result::Result<Value, ToolError> {
        panic!("unsupported source must never call host tool")
    }
}

#[tokio::test]
async fn source_capability_requires_the_owned_native_tool() {
    let root = tempfile::tempdir().expect("workspace");
    let mut tools = ToolManager::new();
    tools.register(Box::new(HostSkill));
    let host = builder(root.path(), "host-tools", Arc::new(Backend::default()))
        .with_tool_manager(tools)
        .build()
        .await
        .expect("host session");
    let restricted = builder(root.path(), "profile-tools", Arc::new(Backend::default()))
        .with_profile(RuntimeSessionProfile::HeadlessCodingV1)
        .build()
        .await
        .expect("profile session");
    let disabled = builder(root.path(), "disabled-tools", Arc::new(Backend::default()))
        .build()
        .await
        .expect("disabled session");
    disabled
        .disable_tools()
        .await
        .expect("disable native tools");
    for session in [host, restricted, disabled] {
        assert!(matches!(
            session
                .apply_skill_source(registration(), origin(&session))
                .await,
            Err(RuntimeSessionError::UnsupportedSource)
        ));
        session.shutdown().await.expect("shutdown");
    }
}
