use std::future::Future;
use std::io::{Read, Write};

use rara_app_server::runtime_control::{RuntimeControlRequest, SessionControlRequest};
use rara_app_server::stdio_protocol::{
    Acknowledgement, Capabilities, ClientFrame, Handshake, PROTOCOL_VERSION, ReceiptCapability,
    RejectionCode, ReplayCapability, ReplayGap, RequestResult, TRANSPORT,
};
use tokio::task::JoinSet;

use super::dispatch::{self, DispatchError};
use super::io::{Failure, Frame, IO_TIMEOUT, Output, start_reader, start_writer};
use super::receipts::{Admission, MAX_REQUESTS, Receipts};
use crate::runtime_session::{RuntimeHost, RuntimeSession, RuntimeSessionError, RuntimeSessionId};

pub(super) const EVENT_CAPACITY: usize = 256;
const MAX_SESSIONS: usize = 8;

pub(super) fn handshake() -> Handshake {
    Handshake {
        protocol_version: PROTOCOL_VERSION,
        runtime_version: env!("CARGO_PKG_VERSION").into(),
        runtime_id: uuid::Uuid::new_v4().to_string(),
        transport: TRANSPORT.into(),
        request_families: [
            "session",
            "input",
            "prompt_source",
            "skill_source",
            "output",
            "server",
        ]
        .map(str::to_owned)
        .to_vec(),
        request_methods: [
            "session.create",
            "session.query_state",
            "session.cancel",
            "session.interrupt",
            "input.submit_prompt",
            "input.submit_follow_up",
            "input.answer_user",
            "input.answer_plan",
            "input.answer_shell",
            "prompt_source.register",
            "prompt_source.unregister",
            "prompt_source.query",
            "skill_source.register",
            "skill_source.disable",
            "skill_source.query",
            "output.replay",
            "server.shutdown",
        ]
        .map(str::to_owned)
        .to_vec(),
        event_families: [
            "session",
            "input",
            "assistant",
            "tool",
            "approval",
            "plan",
            "prompt_source",
            "skill",
            "mcp",
            "memory",
            "hook",
            "context",
            "extension",
            "todo",
            "warning",
            "error",
        ]
        .map(str::to_owned)
        .to_vec(),
        capabilities: Capabilities {
            graceful_shutdown: true,
            approval_persistence: false,
            replay: ReplayCapability::Runtime {
                max_events_per_session: EVENT_CAPACITY as u32,
            },
            request_receipts: ReceiptCapability::Runtime {
                max_requests: MAX_REQUESTS as u32,
            },
        },
        provider: None,
        model: None,
    }
}

struct Server {
    runtime_id: String,
    host: RuntimeHost,
    output: Output,
    receipts: Receipts,
    events: JoinSet<Result<(), Failure>>,
}

pub(super) async fn serve<R, W, F, Fut>(
    reader: R,
    writer: W,
    host: RuntimeHost,
    hello: Handshake,
    mut create: F,
) -> Result<(), Failure>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<RuntimeSession>>,
{
    hello.validate().map_err(|_| Failure::Runtime)?;
    let (output, mut writer_done) = start_writer(writer)?;
    output
        .flush_handshake(Frame::Handshake(hello.clone()))
        .await?;
    let mut input = start_reader(reader)?;
    let mut server = Server {
        runtime_id: hello.runtime_id,
        host,
        output,
        receipts: Receipts::default(),
        events: JoinSet::new(),
    };
    let result = loop {
        tokio::select! {
            biased;
            _ = &mut writer_done => break Err(Failure::Output),
            completed = server.events.join_next(), if !server.events.is_empty() => {
                match completed {
                    Some(Ok(Ok(()))) => {},
                    Some(Ok(Err(error))) => break Err(error),
                    Some(Err(_)) => break Err(Failure::Runtime),
                    None => {},
                }
            }
            incoming = input.recv() => {
                let frame = match incoming {
                    Some(Ok(frame)) => frame,
                    Some(Err(error)) => break Err(error),
                    None => break Err(Failure::InputClosed),
                };
                match server.process(frame, &mut create).await {
                    Ok(Some(shutdown)) => break Ok(shutdown),
                    Ok(None) => {},
                    Err(error) => break Err(error),
                }
            }
        }
    };
    // Close new admission while retaining replies for already settled identities.
    let cleanup = if result.is_ok() {
        server.drain(&mut input, &mut writer_done).await
    } else {
        server.events.abort_all();
        server.host.shutdown().await.map_err(|_| Failure::Cleanup)
    };
    drop(input);
    if cleanup.is_err() {
        server.events.abort_all();
    }
    let mut streams = Ok(());
    while let Some(completed) = server.events.join_next().await {
        if result.is_ok() {
            match completed {
                Ok(Ok(())) => {}
                Ok(Err(error)) => streams = Err(error),
                Err(_) => streams = Err(Failure::Runtime),
            }
        }
    }
    let completion = match (result, cleanup, streams) {
        (_, Err(error), _) | (_, _, Err(error)) | (Err(error), _, _) => Err(error),
        (Ok(request_id), Ok(()), Ok(())) => {
            server
                .output
                .send(Frame::ShutdownComplete {
                    runtime_id: server.runtime_id.clone(),
                    request_id,
                })
                .await
        }
    };
    drop(server);
    completion?;
    tokio::time::timeout(IO_TIMEOUT, writer_done)
        .await
        .map_err(|_| Failure::Output)?
        .map_err(|_| Failure::Output)?
}

impl Server {
    async fn drain(
        &mut self,
        input: &mut tokio::sync::mpsc::Receiver<Result<ClientFrame, Failure>>,
        writer_done: &mut tokio::sync::oneshot::Receiver<Result<(), Failure>>,
    ) -> Result<(), Failure> {
        let host = self.host.clone();
        let cleanup = host.shutdown();
        tokio::pin!(cleanup);
        let mut input_open = true;
        let mut writer_open = true;
        let mut failure = None;
        loop {
            tokio::select! {
                biased;
                result = &mut cleanup => {
                    result.map_err(|_| Failure::Cleanup)?;
                    return failure.map_or(Ok(()), Err);
                }
                _ = &mut *writer_done, if writer_open => {
                    writer_open = false;
                    input_open = false;
                    failure = Some(Failure::Output);
                    self.events.abort_all();
                }
                incoming = input.recv(), if input_open => {
                    let result = match incoming {
                        Some(Ok(frame)) => self.reply_while_closing(frame).await,
                        // A half-close after accepted semantic shutdown is compatible with drain.
                        Some(Err(Failure::InputClosed)) | None => { input_open = false; Ok(()) }
                        Some(Err(error)) => Err(error),
                    };
                    if let Err(error) = result {
                        input_open = false;
                        failure = Some(error);
                        self.events.abort_all();
                    }
                }
            }
        }
    }

    async fn reply_while_closing(&mut self, frame: ClientFrame) -> Result<(), Failure> {
        if frame.runtime_id() != self.runtime_id {
            return self
                .ack(
                    frame.request_id(),
                    dispatch::rejection(RejectionCode::StaleRuntime),
                )
                .await;
        }
        let ack = match self.receipts.admit(&frame)? {
            Admission::Duplicate(ack) => ack,
            Admission::Reject(RejectionCode::Overloaded) => {
                return self
                    .ack(
                        frame.request_id(),
                        dispatch::rejection(RejectionCode::Closed),
                    )
                    .await;
            }
            Admission::Reject(code) => {
                return self
                    .ack(frame.request_id(), dispatch::rejection(code))
                    .await;
            }
            Admission::New => {
                let ack = Acknowledgement {
                    runtime_id: self.runtime_id.clone(),
                    request_id: frame.request_id().into(),
                    result: dispatch::rejection(RejectionCode::Closed),
                };
                self.receipts.finish(ack.clone())?;
                ack
            }
        };
        self.output.send(Frame::Ack(ack)).await
    }

    async fn ack(&self, request_id: &str, result: RequestResult) -> Result<(), Failure> {
        self.output
            .send(Frame::Ack(Acknowledgement {
                runtime_id: self.runtime_id.clone(),
                request_id: request_id.into(),
                result,
            }))
            .await
    }

    async fn process<F, Fut>(
        &mut self,
        frame: ClientFrame,
        create: &mut F,
    ) -> Result<Option<String>, Failure>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = anyhow::Result<RuntimeSession>>,
    {
        if frame.runtime_id() != self.runtime_id {
            self.ack(
                frame.request_id(),
                dispatch::rejection(RejectionCode::StaleRuntime),
            )
            .await?;
            return Ok(None);
        }
        match self.receipts.admit(&frame)? {
            Admission::Duplicate(ack) => {
                self.output.send(Frame::Ack(ack)).await?;
                return Ok(None);
            }
            Admission::Reject(code) => {
                self.ack(frame.request_id(), dispatch::rejection(code))
                    .await?;
                return Ok(None);
            }
            Admission::New => {}
        }
        let request_id = frame.request_id().to_owned();
        let mut new_session = None;
        let mut replay = Vec::new();
        let mut gap = None;
        let mut shutdown = false;
        let result = match &frame {
            ClientFrame::Shutdown { .. } => {
                shutdown = true;
                Ok(RequestResult::Accepted {
                    session_id: None,
                    turn_id: None,
                    last_sequence: None,
                })
            }
            ClientFrame::Control {
                envelope,
                expected_turn_id,
                ..
            } => {
                if matches!(
                    envelope.request,
                    RuntimeControlRequest::Session(SessionControlRequest::CreateSession)
                ) {
                    if envelope.provenance.session_id.is_some() {
                        Err(DispatchError::Rejected(RejectionCode::InvalidRequest))
                    } else if self.host.session_ids().await.len() >= MAX_SESSIONS {
                        Err(DispatchError::Rejected(RejectionCode::Overloaded))
                    } else {
                        // A failed construction exposes no raw provider/config diagnostics.
                        let session = create().await.map_err(|_| Failure::Runtime)?;
                        if self.host.insert(session.clone()).await.is_err() {
                            session.shutdown().await.map_err(|_| Failure::Cleanup)?;
                            return Err(Failure::Runtime);
                        }
                        let accepted = RequestResult::Accepted {
                            session_id: Some(session.id().to_string()),
                            turn_id: None,
                            last_sequence: Some(session.snapshot().last_sequence),
                        };
                        new_session = Some(session);
                        Ok(accepted)
                    }
                } else {
                    match envelope.provenance.session_id.as_deref() {
                        None => Err(DispatchError::Rejected(RejectionCode::InvalidRequest)),
                        Some(id) => match self.host.get(&RuntimeSessionId::new(id)).await {
                            Some(session) => {
                                dispatch::control(&session, envelope, expected_turn_id.as_deref())
                                    .await
                            }
                            None => Err(DispatchError::Rejected(RejectionCode::UnknownSession)),
                        },
                    }
                }
            }
            ClientFrame::Replay {
                session_id,
                after_sequence,
                ..
            } => {
                match self
                    .host
                    .get(&RuntimeSessionId::new(session_id.clone()))
                    .await
                {
                    None => Err(DispatchError::Rejected(RejectionCode::UnknownSession)),
                    Some(session) => match session.replay_events(*after_sequence) {
                        Ok(events) => {
                            let latest = events.last().map(|event| event.sequence);
                            replay = events;
                            Ok(RequestResult::Accepted {
                                session_id: Some(session_id.clone()),
                                turn_id: None,
                                last_sequence: latest,
                            })
                        }
                        Err(RuntimeSessionError::ResyncRequired {
                            requested,
                            oldest_available,
                            latest,
                        }) => {
                            gap = Some(ReplayGap {
                                runtime_id: self.runtime_id.clone(),
                                request_id: request_id.clone(),
                                session_id: session_id.clone(),
                                requested_after: requested,
                                oldest_available,
                                latest,
                            });
                            Err(DispatchError::Rejected(RejectionCode::InvalidRequest))
                        }
                        Err(error) => Err(error.into()),
                    },
                }
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(DispatchError::Rejected(code)) => dispatch::rejection(code),
            Err(DispatchError::Fatal(error)) => return Err(error),
        };
        let ack = Acknowledgement {
            runtime_id: self.runtime_id.clone(),
            request_id: request_id.clone(),
            result,
        };
        self.receipts.finish(ack.clone())?;
        self.output.send(Frame::Ack(ack)).await?;
        if let Some(session) = new_session {
            self.forward(session, request_id.clone())?;
        }
        for event in replay {
            self.output
                .send(Frame::Event {
                    runtime_id: self.runtime_id.clone(),
                    event,
                })
                .await?;
        }
        if let Some(gap) = gap {
            self.output.send(Frame::ReplayGap(gap)).await?;
        }
        Ok(shutdown.then_some(request_id))
    }

    fn forward(&mut self, session: RuntimeSession, request_id: String) -> Result<(), Failure> {
        let mut stream = session.subscribe_after(0).map_err(|_| Failure::ReplayGap)?;
        let output = self.output.clone();
        let runtime_id = self.runtime_id.clone();
        self.events.spawn(async move {
            loop {
                match stream.recv().await {
                    Ok(event) => {
                        output
                            .send(Frame::Event {
                                runtime_id: runtime_id.clone(),
                                event,
                            })
                            .await?
                    }
                    Err(RuntimeSessionError::Closed) => return Ok(()),
                    Err(RuntimeSessionError::ResyncRequired {
                        requested,
                        oldest_available,
                        latest,
                    }) => {
                        output
                            .send(Frame::ReplayGap(ReplayGap {
                                runtime_id,
                                request_id,
                                session_id: session.id().to_string(),
                                requested_after: requested,
                                oldest_available,
                                latest,
                            }))
                            .await?;
                        return Err(Failure::ReplayGap);
                    }
                    Err(_) => return Err(Failure::Runtime),
                }
            }
        });
        Ok(())
    }
}
