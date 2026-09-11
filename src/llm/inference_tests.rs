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

// Capture actual HTTP bodies, including tool schemas and provider options.
async fn server(replies: Vec<Reply>) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
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
            requests
                .push(serde_json::from_slice(&bytes[start..start + length]).expect("JSON body"));
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
    assert_eq!(bodies[0]["model"], "summary-model");
    assert_eq!(bodies[1]["model"], "main-model");
    let snapshot = task.snapshot();
    assert_eq!(snapshot.calls[0].purpose, InferencePurpose::Summary);
    assert_eq!(snapshot.attempts.len(), 2);
    assert_eq!(snapshot.attempts[0].model, "summary-model");
    assert_eq!(snapshot.attempts[1].model, "main-model");
    assert!(snapshot.attempts[0].usage.is_some());
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
    assert_eq!(bodies[1]["model"], "deepseek-v4-pro");
    for field in ["tools", "reasoning_effort", "thinking", "tool_choice"] {
        assert_eq!(bodies[0][field], bodies[1][field], "{field}");
    }
    let before = bodies[0]["messages"].as_array().unwrap();
    let after = bodies[1]["messages"].as_array().unwrap();
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
