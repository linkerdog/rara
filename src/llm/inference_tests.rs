use std::time::Duration;

use rara_observability::{InferencePurpose, InferenceStatus, InferenceTask};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{CodexBackend, LlmBackend, LlmTurnMetadata, Message, OpenAiCompatibleBackend};
use crate::config::OpenAiEndpointKind;

enum Reply {
    Json(u16, Value),
    Stream(String),
    Timeout,
}

struct CapturedRequest {
    body: Value,
    authorization: Option<String>,
}

// Capture actual HTTP bodies, including tool schemas and provider options.
async fn server(replies: Vec<Reply>) -> (String, tokio::task::JoinHandle<Vec<CapturedRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("address");
    let handle = tokio::spawn(async move {
        let mut requests = Vec::new();
        for reply in replies {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
                .await
                .expect("request timeout")
                .expect("accept");
            let mut bytes = Vec::new();
            let (start, length) = loop {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).await.expect("read headers");
                assert!(count > 0, "unexpected request EOF");
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&bytes[..end]).expect("headers");
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("length"))
                        })
                        .expect("content length");
                    break (end + 4, length);
                }
            };
            while bytes.len() < start + length {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).await.expect("read body");
                assert!(count > 0, "unexpected body EOF");
                bytes.extend_from_slice(&buffer[..count]);
            }
            let authorization = std::str::from_utf8(&bytes[..start])
                .unwrap()
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("authorization")
                        .then(|| value.trim().to_owned())
                });
            requests.push(CapturedRequest {
                body: serde_json::from_slice(&bytes[start..start + length]).expect("JSON body"),
                authorization,
            });
            let (status, content_type, body) = match reply {
                Reply::Json(status, value) => (status, "application/json", value.to_string()),
                Reply::Stream(body) => (200, "text/event-stream", body),
                Reply::Timeout => {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    continue;
                }
            };
            let response = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        }
        requests
    });
    (format!("http://{address}"), handle)
}

fn usage() -> Value {
    json!({"prompt_tokens": 100, "completion_tokens": 10,
        "prompt_cache_hit_tokens": 80, "prompt_cache_miss_tokens": 20})
}

fn answer() -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": "done"}, "finish_reason": "stop"}], "usage": usage()})
}

fn prompt() -> Vec<Message> {
    vec![Message {
        role: "user".into(),
        content: json!("complete the task"),
    }]
}

#[tokio::test]
async fn inference_http_retries_are_distinct_and_unknown_charges_are_preserved() {
    let (url, requests) = server(vec![Reply::Timeout, Reply::Json(200, answer())]).await;
    let mut backend = OpenAiCompatibleBackend::new_with_endpoint_kind(
        None,
        url,
        "test-model".into(),
        OpenAiEndpointKind::Deepseek,
    )
    .unwrap();
    backend.client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Main);
    let result = backend
        .ask_with_context(
            &prompt(),
            &[],
            LlmTurnMetadata::default().with_inference(call.context()),
        )
        .await;
    assert!(result.is_ok(), "{result:?}");
    call.finish(&result);
    drop(agent);
    assert_eq!(requests.await.unwrap().len(), 2);
    let snapshot = task.snapshot();
    assert!(snapshot.is_terminal());
    assert_eq!(snapshot.attempts.len(), 2);
    assert_eq!(snapshot.attempts[0].status, InferenceStatus::Failed);
    assert!(snapshot.attempts[0].usage.is_none());
    assert_eq!(snapshot.attempts[1].status, InferenceStatus::Succeeded);
    assert!(snapshot.attempts[1].usage_complete);
    assert_eq!(
        snapshot.attempts[1].usage.unwrap().cache_read_tokens,
        Some(80)
    );
}

#[tokio::test]
async fn inference_summary_fallback_records_both_model_identities() {
    let (url, requests) = server(vec![
        Reply::Json(
            404,
            json!({"error": {"code": "model_not_found"}, "usage": usage()}),
        ),
        Reply::Json(200, answer()),
    ])
    .await;
    let backend = OpenAiCompatibleBackend::new_with_endpoint_kind(
        None,
        url,
        "main-model".into(),
        OpenAiEndpointKind::Deepseek,
    )
    .unwrap()
    .with_auxiliary_model(Some("summary-model".into()));
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Summary);
    let result = backend
        .summarize_with_context(
            &prompt(),
            "summarize",
            LlmTurnMetadata::default().with_inference(call.context()),
        )
        .await;
    assert_eq!(result.as_deref().unwrap(), "done");
    call.finish(&result);
    drop(agent);
    let bodies = requests.await.unwrap();
    assert_eq!(bodies[0].body["model"], "summary-model");
    assert_eq!(bodies[1].body["model"], "main-model");
    let snapshot = task.snapshot();
    assert_eq!(snapshot.calls[0].purpose, InferencePurpose::Summary);
    assert_eq!(snapshot.attempts.len(), 2);
    assert_eq!(snapshot.attempts[0].model, "summary-model");
    assert_eq!(snapshot.attempts[1].model, "main-model");
    assert!(snapshot.attempts[0].usage.is_some());
    assert!(snapshot.attempts[0].usage_complete);
    assert!(snapshot.attempts[1].usage_complete);
    assert_eq!(snapshot.attempts[0].status, InferenceStatus::Failed);
}

#[tokio::test]
async fn inference_stream_decode_failure_retains_partial_usage() {
    let stream = format!(
        "data: {}\n\ndata: invalid JSON\n\n",
        json!({"choices": [], "usage": usage()})
    );
    let (url, requests) = server(vec![Reply::Stream(stream)]).await;
    let backend = OpenAiCompatibleBackend::new(None, url, "test-model".into()).unwrap();
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Main);
    let result = backend
        .ask_streaming_with_context(
            &prompt(),
            &[],
            LlmTurnMetadata::default().with_inference(call.context()),
            &mut |_| {},
        )
        .await;
    assert!(result.is_err());
    call.finish(&result);
    drop(agent);
    requests.await.unwrap();
    let attempt = &task.snapshot().attempts[0];
    assert_eq!(attempt.status, InferenceStatus::Failed);
    assert_eq!(attempt.usage.unwrap().input_tokens, 100);
    assert!(!attempt.usage_complete);
}

#[tokio::test]
async fn inference_responses_terminal_usage_survives_later_decode_failure() {
    let stream = format!(
        "data: {}\n\ndata: invalid JSON\n\n",
        json!({"type": "response.completed", "response": {"usage": {
            "input_tokens": 100, "output_tokens": 10,
            "input_tokens_details": {"cached_tokens": 80, "cache_write_tokens": 20}
        }}})
    );
    let (url, requests) = server(vec![Reply::Stream(stream)]).await;
    let backend = CodexBackend::new(None, url, "test-model".into(), None).unwrap();
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Main);
    let result = backend
        .ask_with_context(
            &prompt(),
            &[],
            LlmTurnMetadata::default().with_inference(call.context()),
        )
        .await;
    assert!(result.is_err());
    call.finish(&result);
    drop(agent);
    requests.await.unwrap();
    let attempt = &task.snapshot().attempts[0];
    assert_eq!(attempt.status, InferenceStatus::Failed);
    assert!(attempt.usage_complete);
    assert_eq!(attempt.usage.unwrap().cache_write_tokens, Some(20));
}

#[tokio::test]
async fn cached_summary_reuses_wire_prefix_tools_model_and_reasoning_options() {
    let (url, requests) =
        server(vec![Reply::Json(200, answer()), Reply::Json(200, answer())]).await;
    let backend = OpenAiCompatibleBackend::new_with_endpoint_kind_and_reasoning(
        None,
        url,
        "deepseek-v4-pro".into(),
        OpenAiEndpointKind::Deepseek,
        Some("high".into()),
        Some(true),
    )
    .unwrap()
    .with_auxiliary_model(Some("deepseek-v4-flash".into()));
    let mut manager = rara_tools::tool::ToolManager::new();
    manager.register(Box::<rara_tools::file::ReadFileTool>::default());
    manager.register(Box::<rara_tools::file::WriteFileTool>::default());
    let tools = manager.get_schemas();
    let mut messages = vec![Message {
        role: "system".into(),
        content: json!("stable instructions"),
    }];
    messages.extend(prompt());
    backend
        .ask_with_context(&messages, &tools, LlmTurnMetadata::plan())
        .await
        .unwrap();
    let prefix = super::SummaryPrefix {
        messages: messages.clone(),
        tools,
        execution_mode: super::LlmExecutionMode::Plan,
    };
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Summary);
    let result = backend
        .summarize_with_prefix(
            &messages[1..],
            "summarize",
            &prefix,
            LlmTurnMetadata::execute().with_inference(call.context()),
        )
        .await;
    assert_eq!(result.as_deref().unwrap(), "done");
    call.finish(&result);
    drop(agent);
    let bodies = requests.await.unwrap();
    assert_eq!(bodies[1].body["model"], "deepseek-v4-pro");
    for field in ["tools", "reasoning_effort", "thinking", "tool_choice"] {
        assert_eq!(bodies[0].body[field], bodies[1].body[field], "{field}");
    }
    let before = bodies[0].body["messages"].as_array().unwrap();
    let after = bodies[1].body["messages"].as_array().unwrap();
    assert_eq!(&after[..before.len()], before.as_slice());
    assert_eq!(task.snapshot().attempts[0].model, "deepseek-v4-pro");
}

#[tokio::test]
async fn cached_summary_rejects_generated_tools_without_executing_them() {
    let response = json!({"choices": [{"message": {"role": "assistant", "tool_calls": [{"id": "write", "type": "function", "function": {"name": "write_file", "arguments": "{\"path\":\"forbidden\",\"content\":\"bad\"}"}}]}, "finish_reason": "tool_calls"}], "usage": usage()});
    let (url, requests) = server(vec![Reply::Json(200, response)]).await;
    let backend = OpenAiCompatibleBackend::new(None, url, "test-model".into()).unwrap();
    let mut messages = vec![Message {
        role: "system".into(),
        content: json!("stable"),
    }];
    messages.extend(prompt());
    let prefix = super::SummaryPrefix {
        messages: messages.clone(),
        tools: vec![],
        execution_mode: super::LlmExecutionMode::Execute,
    };
    let result = backend
        .summarize_with_prefix(
            &messages[1..],
            "summarize",
            &prefix,
            LlmTurnMetadata::default(),
        )
        .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("no tool was executed")
    );
    assert_eq!(requests.await.unwrap().len(), 1);
}

#[tokio::test]
async fn inference_status_retries_preserve_responses_authentication_and_usage() {
    let stream = format!(
        "data: {}\n\n",
        json!({"type":"response.completed", "response": {
            "output": [], "usage": {"input_tokens":100,"output_tokens":10,
            "input_tokens_details":{"cached_tokens":80,"cache_write_tokens":0}}
        }})
    );
    let (url, requests) = server(vec![
        Reply::Json(429, json!({"error":{"message":"busy"},"usage":usage()})),
        Reply::Json(503, json!({"error":{"message":"unavailable"}})),
        Reply::Stream(stream),
    ])
    .await;
    let backend = CodexBackend::new(
        Some("fixture-response-key".into()),
        url,
        "test-model".into(),
        None,
    )
    .unwrap();
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Main);
    let result = backend
        .ask_with_context(
            &prompt(),
            &[],
            LlmTurnMetadata::default().with_inference(call.context()),
        )
        .await;
    assert!(result.is_ok(), "{result:?}");
    call.finish(&result);
    drop(agent);
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(
            request.authorization.as_deref(),
            Some("Bearer fixture-response-key")
        );
        assert_eq!(request.body, requests[0].body);
    }
    let snapshot = task.snapshot();
    assert!(snapshot.is_terminal());
    assert_eq!(snapshot.attempts.len(), 3);
    assert_eq!(snapshot.attempts[0].status, InferenceStatus::Failed);
    assert_eq!(snapshot.attempts[0].usage.unwrap().input_tokens, 100);
    assert!(snapshot.attempts[0].usage_complete);
    assert_eq!(snapshot.attempts[1].status, InferenceStatus::Failed);
    assert!(snapshot.attempts[1].usage.is_none());
    assert!(snapshot.attempts[2].usage_complete);
}

#[tokio::test]
async fn inference_status_retries_cover_chat_and_auxiliary_summaries() {
    for purpose in [InferencePurpose::Main, InferencePurpose::Summary] {
        let (url, requests) = server(vec![
            Reply::Json(503, json!({"usage":usage()})),
            Reply::Json(200, answer()),
        ])
        .await;
        let backend = OpenAiCompatibleBackend::new_with_endpoint_kind(
            None,
            url,
            "main-model".into(),
            OpenAiEndpointKind::Deepseek,
        )
        .unwrap()
        .with_auxiliary_model(Some("summary-model".into()));
        let task = InferenceTask::default();
        let agent = task.start_agent(None);
        let call = agent.start_call(purpose);
        let metadata = LlmTurnMetadata::default().with_inference(call.context());
        let result = match purpose {
            InferencePurpose::Main => backend
                .ask_with_context(&prompt(), &[], metadata)
                .await
                .map(|_| ()),
            InferencePurpose::Summary => backend
                .summarize_with_context(&prompt(), "summarize", metadata)
                .await
                .map(|_| ()),
            InferencePurpose::Classifier => unreachable!(),
        };
        assert!(result.is_ok(), "{result:?}");
        call.finish(&result);
        drop(agent);
        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].body, requests[1].body);
        let snapshot = task.snapshot();
        assert_eq!(snapshot.attempts.len(), 2);
        assert!(
            snapshot
                .attempts
                .iter()
                .all(|attempt| attempt.usage_complete)
        );
        assert_eq!(snapshot.attempts[0].status, InferenceStatus::Failed);
    }
}

#[tokio::test]
async fn inference_status_retries_stop_at_the_physical_attempt_bound() {
    let replies = (0..=super::inference_transport::MAX_SEND_RETRIES)
        .map(|_| Reply::Json(429, json!({"usage":usage()})))
        .collect();
    let (url, requests) = server(replies).await;
    let backend = OpenAiCompatibleBackend::new(None, url, "test-model".into()).unwrap();
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Main);
    let result = backend
        .ask_with_context(
            &prompt(),
            &[],
            LlmTurnMetadata::default().with_inference(call.context()),
        )
        .await;
    assert!(result.is_err());
    call.finish(&result);
    drop(agent);
    assert_eq!(
        requests.await.unwrap().len(),
        super::inference_transport::MAX_SEND_RETRIES + 1
    );
    let snapshot = task.snapshot();
    assert_eq!(
        snapshot.attempts.len(),
        super::inference_transport::MAX_SEND_RETRIES + 1
    );
    assert!(
        snapshot
            .attempts
            .iter()
            .all(|attempt| attempt.status == InferenceStatus::Failed && attempt.usage_complete)
    );
}

#[test]
fn cached_summary_keeps_system_messages_within_captured_history() {
    let prefix = super::SummaryPrefix {
        messages: vec![
            Message {
                role: "system".into(),
                content: json!("stable instructions"),
            },
            Message {
                role: "system".into(),
                content: json!("previous compact summary"),
            },
            Message {
                role: "user".into(),
                content: json!("continue the review"),
            },
        ],
        tools: vec![],
        execution_mode: super::LlmExecutionMode::Execute,
    };
    let request = prefix
        .messages_for_summary(&prefix.messages[1..], "summarize")
        .unwrap();
    assert_eq!(&request[..prefix.messages.len()], &prefix.messages);
    assert_eq!(request.len(), prefix.messages.len() + 1);
    assert!(!prefix.matches_history(&prefix.messages[2..]));
}

#[test]
fn cached_summary_requires_the_entire_captured_history_prefix() {
    let messages = vec![
        Message {
            role: "system".into(),
            content: json!("stable"),
        },
        Message {
            role: "user".into(),
            content: json!("first"),
        },
        Message {
            role: "assistant".into(),
            content: json!("evidence"),
        },
        Message {
            role: "user".into(),
            content: json!("second"),
        },
    ];
    let prefix = super::SummaryPrefix {
        messages: messages.clone(),
        tools: vec![],
        execution_mode: super::LlmExecutionMode::Execute,
    };
    for length in 0..messages.len() - 1 {
        assert!(!prefix.matches_history(&messages[1..1 + length]));
        assert!(
            prefix
                .messages_for_summary(&messages[1..1 + length], "summarize")
                .is_err()
        );
    }
    assert!(prefix.matches_history(&messages[1..]));
    let mut extended = messages[1..].to_vec();
    extended.push(Message {
        role: "assistant".into(),
        content: json!("more evidence"),
    });
    assert!(prefix.matches_history(&extended));
    assert!(!prefix.matches_history(&messages[2..]));
}
