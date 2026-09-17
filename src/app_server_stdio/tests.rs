use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use rara_app_server::runtime_control::{
    InputControlRequest, RuntimeControlEnvelope, RuntimeControlRequest, RuntimeControllerKind,
    RuntimeProvenance, RuntimeSourceTrust, SessionControlRequest, SkillSourceControlRequest,
};
use rara_app_server::stdio_protocol::{Acknowledgement, ClientFrame, RequestResult};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Notify;
use tokio::time::timeout;

use super::{
    io::{Failure, Frame},
    server,
};
use crate::runtime_control::SkillEvent;
use crate::{
    ContentBlock, LlmBackend, LlmResponse, Message, RaraConfig, RuntimeEvent, RuntimeHost,
    RuntimeSessionBuilder, RuntimeSessionId, RuntimeSessionPhase, SessionEvent,
};

mod framing_receipts;

const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Backend {
    calls: AtomicUsize,
    started: Notify,
    gate: Option<Notify>,
}

#[async_trait]
impl LlmBackend for Backend {
    async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "fixture done".into(),
            }],
            stop_reason: Some("end_turn".into()),
            usage: None,
        })
    }
    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        anyhow::bail!("transport fixture must not summarize")
    }
}

struct Writer {
    socket: std::os::unix::net::UnixStream,
    fail: Arc<AtomicBool>,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.flush()
    }
}

struct Harness {
    runtime_id: String,
    input: OwnedWriteHalf,
    output: BufReader<OwnedReadHalf>,
    task: tokio::task::JoinHandle<std::result::Result<(), Failure>>,
    host: RuntimeHost,
    observed: Vec<Frame>,
    fail_writer: Arc<AtomicBool>,
}

impl Harness {
    async fn start(root: &std::path::Path, backend: Arc<Backend>) -> Self {
        let (client, socket) = std::os::unix::net::UnixStream::pair().expect("socket pair");
        client.set_nonblocking(true).expect("async client");
        let client = tokio::net::UnixStream::from_std(client).expect("client");
        let (read, input) = client.into_split();
        let host = RuntimeHost::new();
        let owner = host.clone();
        let root = root.to_path_buf();
        let reader = socket.try_clone().expect("reader");
        let fail_writer = Arc::new(AtomicBool::new(false));
        let writer = Writer {
            socket,
            fail: fail_writer.clone(),
        };
        let task = tokio::spawn(async move {
            server::serve(reader, writer, owner, server::handshake(), move || {
                RuntimeSessionBuilder::new(RaraConfig::default(), &root)
                    .with_backend(backend.clone())
                    .with_state_root(root.join("state"))
                    .without_extension_discovery()
                    .without_memory_facilities()
                    .without_transcript_persistence()
                    .with_event_capacity(server::EVENT_CAPACITY)
                    .build()
            })
            .await
        });
        let mut harness = Self {
            runtime_id: String::new(),
            input,
            output: BufReader::new(read),
            task,
            host,
            observed: Vec::new(),
            fail_writer,
        };
        let Frame::Handshake(hello) = harness.next().await else {
            panic!("handshake must be first");
        };
        hello.validate().expect("valid handshake");
        assert!(!hello.capabilities.approval_persistence);
        assert!(
            !hello
                .request_methods
                .iter()
                .any(|method| method.contains("resume"))
        );
        harness.runtime_id = hello.runtime_id;
        harness
    }

    fn control(
        &self,
        id: &str,
        session: Option<&str>,
        request: RuntimeControlRequest,
    ) -> ClientFrame {
        ClientFrame::Control {
            runtime_id: self.runtime_id.clone(),
            expected_turn_id: None,
            envelope: Box::new(RuntimeControlEnvelope {
                request_id: id.into(),
                provenance: RuntimeProvenance::runtime(session.map(str::to_owned)),
                request,
            }),
        }
    }

    async fn send(&mut self, frame: &ClientFrame) {
        let mut bytes = serde_json::to_vec(frame).expect("encode request");
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.expect("send request");
    }

    async fn next(&mut self) -> Frame {
        let mut line = String::new();
        let count = timeout(DEADLINE, self.output.read_line(&mut line))
            .await
            .expect("frame timeout")
            .expect("frame read");
        assert_ne!(count, 0, "unexpected output EOF");
        let frame: Frame = serde_json::from_str(&line).expect("protocol-only output");
        self.observed.push(frame.clone());
        frame
    }

    async fn ack(&mut self, request_id: &str) -> Acknowledgement {
        loop {
            if let Frame::Ack(ack) = self.next().await
                && ack.request_id == request_id
            {
                return ack;
            }
        }
    }

    async fn create(&mut self) -> String {
        let frame = self.control(
            "create",
            None,
            RuntimeControlRequest::Session(SessionControlRequest::CreateSession),
        );
        self.send(&frame).await;
        let RequestResult::Accepted {
            session_id: Some(id),
            ..
        } = self.ack("create").await.result
        else {
            panic!("session creation");
        };
        id
    }

    async fn shutdown(mut self) {
        let frame = ClientFrame::Shutdown {
            runtime_id: self.runtime_id.clone(),
            request_id: "shutdown".into(),
        };
        self.send(&frame).await;
        assert!(matches!(
            self.ack("shutdown").await.result,
            RequestResult::Accepted { .. }
        ));
        loop {
            if let Frame::ShutdownComplete { request_id, .. } = self.next().await {
                assert_eq!(request_id, "shutdown");
                break;
            }
        }
        // Keep the input half open until serve returns: no dependency on stdin EOF.
        timeout(DEADLINE, self.task)
            .await
            .expect("shutdown timeout")
            .expect("server task")
            .expect("semantic shutdown");
        assert!(self.host.session_ids().await.is_empty());
    }
}

#[tokio::test]
async fn transport_retains_receipts_and_replays_canonical_events() {
    let root = tempfile::tempdir().expect("workspace");
    let backend = Arc::new(Backend::default());
    let mut client = Harness::start(root.path(), backend.clone()).await;
    let mut stale = client.control(
        "create",
        None,
        RuntimeControlRequest::Session(SessionControlRequest::CreateSession),
    );
    if let ClientFrame::Control { runtime_id, .. } = &mut stale {
        *runtime_id = "old-runtime".into();
    }
    client.send(&stale).await;
    assert!(matches!(
        client.ack("create").await.result,
        RequestResult::Rejected {
            code: rara_app_server::stdio_protocol::RejectionCode::StaleRuntime,
            ..
        }
    ));
    assert!(client.host.session_ids().await.is_empty());
    let id = client.create().await;
    let create = client.control(
        "create",
        None,
        RuntimeControlRequest::Session(SessionControlRequest::CreateSession),
    );
    client.send(&create).await;
    assert!(
        matches!(client.ack("create").await.result, RequestResult::Accepted { session_id: Some(repeated), .. } if repeated == id)
    );
    assert_eq!(client.host.session_ids().await.len(), 1);
    let conflict = client.control(
        "create",
        Some(&id),
        RuntimeControlRequest::Session(SessionControlRequest::QueryRuntimeState),
    );
    client.send(&conflict).await;
    assert!(matches!(
        client.ack("create").await.result,
        RequestResult::Rejected {
            code: rara_app_server::stdio_protocol::RejectionCode::RequestConflict,
            ..
        }
    ));

    let prompt = client.control(
        "prompt",
        Some(&id),
        RuntimeControlRequest::Input(InputControlRequest::SubmitUserPrompt {
            prompt: "one invocation".into(),
        }),
    );
    client.send(&prompt).await;
    let accepted = client.ack("prompt").await;
    client.send(&prompt).await;
    assert_eq!(client.ack("prompt").await, accepted);
    loop {
        if client.observed.iter().any(|frame| matches!(frame, Frame::Event { event, .. } if matches!(event.event, RuntimeEvent::Session(SessionEvent::TurnFinished { .. })))) { break; }
        client.next().await;
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    client.send(&prompt).await;
    assert_eq!(client.ack("prompt").await, accepted);

    let register = client.control(
        "skill",
        Some(&id),
        RuntimeControlRequest::SkillSource(SkillSourceControlRequest::RegisterSkill {
            source_id: "wire-source".into(),
            name: "wire-skill".into(),
            content: "# Skill\nCompact summary.\n\nPrivate instructions.".into(),
            precedence_hint: None,
        }),
    );
    client.send(&register).await;
    assert!(matches!(
        client.ack("skill").await.result,
        RequestResult::Accepted { .. }
    ));
    let state = client.control(
        "state",
        Some(&id),
        RuntimeControlRequest::Session(SessionControlRequest::QueryRuntimeState),
    );
    client.send(&state).await;
    client.ack("state").await;
    loop {
        if client.observed.iter().any(|frame| matches!(frame, Frame::Event { event, .. } if matches!(event.event, RuntimeEvent::Session(SessionEvent::RuntimeState { .. })))) { break; }
        client.next().await;
    }
    let registered = client
        .observed
        .iter()
        .find_map(|frame| match frame {
            Frame::Event { event, .. }
                if matches!(
                    event.event,
                    RuntimeEvent::Skill(SkillEvent::Registered { .. })
                ) =>
            {
                Some(event)
            }
            _ => None,
        })
        .expect("registered event");
    assert_eq!(
        registered.provenance.controller,
        RuntimeControllerKind::AppServer
    );
    assert_eq!(registered.provenance.trust, RuntimeSourceTrust::Untrusted);
    assert_eq!(
        registered.provenance.session_id.as_deref(),
        Some(id.as_str())
    );
    let original = client
        .observed
        .iter()
        .filter_map(|frame| match frame {
            Frame::Event { event, .. } => Some(event.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let replay = ClientFrame::Replay {
        runtime_id: client.runtime_id.clone(),
        request_id: "replay".into(),
        session_id: id.clone(),
        after_sequence: 0,
    };
    client.send(&replay).await;
    let RequestResult::Accepted {
        last_sequence: Some(last),
        ..
    } = client.ack("replay").await.result
    else {
        panic!("replay accepted");
    };
    let mut replayed = Vec::new();
    loop {
        if let Frame::Event { event, .. } = client.next().await {
            let done = event.sequence == last;
            replayed.push(event);
            if done {
                break;
            }
        }
    }
    assert_eq!(replayed, original);
    let gap = ClientFrame::Replay {
        runtime_id: client.runtime_id.clone(),
        request_id: "gap".into(),
        session_id: id,
        after_sequence: u64::MAX,
    };
    client.send(&gap).await;
    assert!(matches!(
        client.ack("gap").await.result,
        RequestResult::Rejected { .. }
    ));
    assert!(matches!(client.next().await, Frame::ReplayGap(_)));
    client.shutdown().await;
}

#[tokio::test]
async fn input_loss_drains_active_execution_without_claiming_shutdown() {
    let root = tempfile::tempdir().expect("workspace");
    let backend = Arc::new(Backend {
        gate: Some(Notify::new()),
        ..Default::default()
    });
    let mut client = Harness::start(root.path(), backend.clone()).await;
    let id = client.create().await;
    let session = client
        .host
        .get(&RuntimeSessionId::new(id.clone()))
        .await
        .expect("owned session");
    let prompt = client.control(
        "held",
        Some(&id),
        RuntimeControlRequest::Input(InputControlRequest::SubmitUserPrompt {
            prompt: "held".into(),
        }),
    );
    client.send(&prompt).await;
    client.ack("held").await;
    timeout(DEADLINE, backend.started.notified())
        .await
        .expect("provider started");
    client.input.shutdown().await.expect("input EOF");
    let mut snapshots = session.subscribe_snapshots();
    timeout(DEADLINE, async {
        while !matches!(snapshots.borrow().phase, RuntimeSessionPhase::Closing) {
            snapshots.changed().await.expect("snapshot");
        }
    })
    .await
    .expect("cleanup begins");
    assert!(!client.task.is_finished());
    backend.gate.as_ref().expect("gate").notify_one();
    assert!(matches!(
        timeout(DEADLINE, client.task)
            .await
            .expect("cleanup timeout")
            .expect("server task"),
        Err(Failure::InputClosed)
    ));
    assert!(client.host.session_ids().await.is_empty());
    assert!(matches!(
        session.snapshot().phase,
        RuntimeSessionPhase::Closed
    ));
}

#[tokio::test]
async fn malformed_input_and_writer_failure_release_owned_sessions() {
    for output_failure in [false, true] {
        let root = tempfile::tempdir().expect("workspace");
        let mut client = Harness::start(root.path(), Arc::new(Backend::default())).await;
        let id = client.create().await;
        if output_failure {
            client.fail_writer.store(true, Ordering::SeqCst);
            let query = client.control(
                "state",
                Some(&id),
                RuntimeControlRequest::Session(SessionControlRequest::QueryRuntimeState),
            );
            client.send(&query).await;
        } else {
            client
                .input
                .write_all(b"private-malformed-data\n")
                .await
                .expect("malformed input");
        }
        let result = timeout(DEADLINE, client.task)
            .await
            .expect("cleanup timeout")
            .expect("server task");
        assert!(matches!(result, Err(Failure::Output | Failure::Framing)));
        assert!(client.host.session_ids().await.is_empty());
        assert!(
            !client
                .observed
                .iter()
                .any(|frame| matches!(frame, Frame::ShutdownComplete { .. }))
        );
    }
}

#[tokio::test]
async fn wire_turn_targets_reject_stale_stops_and_preserve_native_cancellation() {
    let root = tempfile::tempdir().expect("workspace");
    let backend = Arc::new(Backend {
        gate: Some(Notify::new()),
        ..Default::default()
    });
    let mut client = Harness::start(root.path(), backend.clone()).await;
    let id = client.create().await;
    let missing = client.control(
        "missing",
        Some("not-owned"),
        RuntimeControlRequest::Session(SessionControlRequest::QueryRuntimeState),
    );
    client.send(&missing).await;
    assert!(matches!(
        client.ack("missing").await.result,
        RequestResult::Rejected {
            code: rara_app_server::stdio_protocol::RejectionCode::UnknownSession,
            ..
        }
    ));
    let resume = client.control(
        "resume",
        Some(&id),
        RuntimeControlRequest::Session(SessionControlRequest::ResumeSession {
            session_id: id.clone(),
        }),
    );
    client.send(&resume).await;
    assert!(matches!(
        client.ack("resume").await.result,
        RequestResult::Rejected {
            code: rara_app_server::stdio_protocol::RejectionCode::Unsupported,
            ..
        }
    ));
    let prompt = client.control(
        "held",
        Some(&id),
        RuntimeControlRequest::Input(InputControlRequest::SubmitUserPrompt {
            prompt: "held".into(),
        }),
    );
    client.send(&prompt).await;
    let RequestResult::Accepted {
        turn_id: Some(turn),
        ..
    } = client.ack("held").await.result
    else {
        panic!("turn identity");
    };
    timeout(DEADLINE, backend.started.notified())
        .await
        .expect("started");
    for (request_id, expected, rejected) in [
        ("stale", "older-turn", true),
        ("cancel", turn.as_str(), false),
    ] {
        let mut stop = client.control(
            request_id,
            Some(&id),
            RuntimeControlRequest::Session(SessionControlRequest::CancelCurrentTurn),
        );
        if let ClientFrame::Control {
            expected_turn_id, ..
        } = &mut stop
        {
            *expected_turn_id = Some(expected.into());
        }
        client.send(&stop).await;
        let ack = client.ack(request_id).await;
        if rejected {
            assert!(matches!(
                ack.result,
                RequestResult::Rejected {
                    code: rara_app_server::stdio_protocol::RejectionCode::InvalidRequest,
                    ..
                }
            ));
        } else {
            assert!(
                matches!(ack.result, RequestResult::Accepted { turn_id: Some(id), .. } if id == turn)
            );
        }
    }
    backend.gate.as_ref().expect("gate").notify_one();
    loop {
        if let Frame::Event { event, .. } = client.next().await
            && matches!(
                event.event,
                RuntimeEvent::Session(SessionEvent::TurnCancelled)
            )
        {
            assert_eq!(event.turn_id.as_deref(), Some(turn.as_str()));
            break;
        }
    }
    client.shutdown().await;
}

#[tokio::test]
async fn closing_replays_receipts_while_rejecting_new_work() {
    let root = tempfile::tempdir().expect("workspace");
    let backend = Arc::new(Backend {
        gate: Some(Notify::new()),
        ..Default::default()
    });
    let mut client = Harness::start(root.path(), backend.clone()).await;
    let id = client.create().await;
    let prompt = client.control(
        "held",
        Some(&id),
        RuntimeControlRequest::Input(InputControlRequest::SubmitUserPrompt {
            prompt: "held".into(),
        }),
    );
    client.send(&prompt).await;
    let prompt_ack = client.ack("held").await;
    timeout(DEADLINE, backend.started.notified())
        .await
        .expect("provider started");
    let shutdown = ClientFrame::Shutdown {
        runtime_id: client.runtime_id.clone(),
        request_id: "shutdown".into(),
    };
    client.send(&shutdown).await;
    let shutdown_ack = client.ack("shutdown").await;
    assert!(matches!(
        shutdown_ack.result,
        RequestResult::Accepted { .. }
    ));
    client.send(&shutdown).await;
    assert_eq!(client.ack("shutdown").await, shutdown_ack);
    client.send(&prompt).await;
    assert_eq!(client.ack("held").await, prompt_ack);
    let new = client.control(
        "new",
        Some(&id),
        RuntimeControlRequest::Input(InputControlRequest::SubmitFollowUp {
            prompt: "must not run".into(),
        }),
    );
    client.send(&new).await;
    assert!(matches!(
        client.ack("new").await.result,
        RequestResult::Rejected {
            code: rara_app_server::stdio_protocol::RejectionCode::Closed,
            ..
        }
    ));
    for index in 0..super::receipts::MAX_REQUESTS {
        let request_id = format!("closing-{index}");
        let request = client.control(
            &request_id,
            Some(&id),
            RuntimeControlRequest::Session(SessionControlRequest::QueryRuntimeState),
        );
        client.send(&request).await;
        assert!(matches!(
            client.ack(&request_id).await.result,
            RequestResult::Rejected {
                code: rara_app_server::stdio_protocol::RejectionCode::Closed,
                ..
            }
        ));
    }
    client.send(&shutdown).await;
    assert_eq!(client.ack("shutdown").await, shutdown_ack);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    backend.gate.as_ref().expect("gate").notify_one();
    loop {
        if matches!(client.next().await, Frame::ShutdownComplete { .. }) {
            break;
        }
    }
    timeout(DEADLINE, client.task)
        .await
        .expect("shutdown timeout")
        .expect("task")
        .expect("shutdown");
    assert!(client.host.session_ids().await.is_empty());
}
