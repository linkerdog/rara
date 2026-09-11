use rara_observability::{InferencePurpose, InferenceStatus, InferenceTask};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

#[tokio::test]
async fn sdk_retries_are_recorded_and_converse_checkpoints_reach_the_wire() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for status in [429, 200] {
            let (mut socket, _) =
                tokio::time::timeout(std::time::Duration::from_secs(15), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut bytes = Vec::new();
            let (start, length) = loop {
                let mut buffer = [0_u8; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&bytes[..end]).unwrap();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < start + length {
                let mut buffer = [0_u8; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            requests.push(serde_json::from_slice::<Value>(&bytes[start..start + length]).unwrap());
            let body = if status == 429 {
                json!({"message": "retry", "__type": "ThrottlingException"})
            } else {
                json!({"output": {"message": {"role": "assistant", "content": [{"text": "done"}]}},
                    "stopReason": "end_turn", "usage": {"inputTokens": 10, "outputTokens": 5, "totalTokens": 115,
                        "cacheReadInputTokens": 80, "cacheWriteInputTokens": 20,
                        "cacheDetails": [{"ttl": "5m", "inputTokens": 20}]}})
            }.to_string();
            let response = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
        requests
    });
    let config = aws_sdk_bedrockruntime::Config::builder()
        .behavior_version_latest()
        .region(aws_sdk_bedrockruntime::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_bedrockruntime::config::Credentials::new(
            "fixture", "fixture", None, None, "fixture",
        ))
        .retry_config(
            aws_sdk_bedrockruntime::config::retry::RetryConfig::standard().with_max_attempts(2),
        )
        .endpoint_url(endpoint)
        .build();
    let client = BedrockConverseClient {
        client: BedrockClient::from_conf(config),
        model_id: "anthropic.claude-sonnet-4-6".into(),
        region: "us-east-1".into(),
    };
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Main);
    let result = client
        .ask_with_options(
            &["stable rules".into()],
            &[BedrockChatMessage {
                role: BedrockChatRole::User,
                content: vec![BedrockChatContent::Text("task".into())],
            }],
            &[],
            BedrockRequestOptions {
                cache_ttl: Some(BedrockCacheTtl::FiveMinutes),
                inference: Some(call.context()),
            },
        )
        .await;
    assert!(result.is_ok(), "{result:?}");
    call.finish(&result);
    drop(agent);
    let bodies = server.await.unwrap();
    assert_eq!(bodies[0], bodies[1]);
    assert_eq!(bodies[0]["system"][1]["cachePoint"]["ttl"], "5m");
    assert_eq!(
        bodies[0]["messages"][0]["content"][1]["cachePoint"]["type"],
        "default"
    );
    let snapshot = task.snapshot();
    assert!(snapshot.is_terminal());
    assert_eq!(snapshot.attempts.len(), 2);
    assert_eq!(snapshot.attempts[0].status, InferenceStatus::Failed);
    assert_eq!(snapshot.attempts[1].status, InferenceStatus::Succeeded);
    assert_eq!(snapshot.attempts[1].usage.unwrap().input_tokens, 110);
    assert_eq!(
        snapshot.attempts[1].usage.unwrap().cache_write_5m_tokens,
        Some(20)
    );
}
