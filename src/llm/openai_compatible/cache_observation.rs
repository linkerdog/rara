use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::config::OpenAiEndpointKind;
use crate::model_observation::ModelRequestFingerprint;

const MAX_COMPONENT_FINGERPRINTS: usize = 256;

pub(super) fn enable_streaming_usage(body: &mut Value, endpoint_kind: OpenAiEndpointKind) {
    body["stream"] = json!(true);
    if endpoint_kind == OpenAiEndpointKind::Deepseek {
        body["stream_options"] = json!({ "include_usage": true });
    }
}

pub(super) fn apply_deepseek_user_id(
    body: &mut Value,
    endpoint_kind: OpenAiEndpointKind,
    user_id: Option<&str>,
) {
    if endpoint_kind == OpenAiEndpointKind::Deepseek
        && let Some(user_id) = user_id.filter(|user_id| !user_id.is_empty())
    {
        body["user_id"] = json!(user_id);
    }
}

pub(super) fn fingerprint_request(
    body: &Value,
    hash_scope: &str,
    hash_salt: &[u8],
) -> ModelRequestFingerprint {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let tools = body
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let system_messages = messages
        .iter()
        .take_while(|message| message.get("role").and_then(Value::as_str) == Some("system"))
        .cloned()
        .collect::<Vec<_>>();

    let mut logical_request = body.clone();
    remove_transport_fields(&mut logical_request);

    let mut options = logical_request.clone();
    if let Some(options) = options.as_object_mut() {
        options.remove("messages");
        options.remove("tools");
    }

    ModelRequestFingerprint {
        version: 1,
        hash_scope: hash_scope.to_string(),
        request_sha256: sha256_json(&logical_request, hash_salt),
        system_sha256: (!system_messages.is_empty())
            .then(|| sha256_json(&Value::Array(system_messages), hash_salt)),
        messages_sha256: sha256_json(&Value::Array(messages.clone()), hash_salt),
        tools_sha256: sha256_json(&Value::Array(tools.clone()), hash_salt),
        options_sha256: sha256_json(&options, hash_salt),
        message_sha256: messages
            .iter()
            .take(MAX_COMPONENT_FINGERPRINTS)
            .map(|message| sha256_json(message, hash_salt))
            .collect(),
        tool_sha256: tools
            .iter()
            .take(MAX_COMPONENT_FINGERPRINTS)
            .map(|tool| sha256_json(tool, hash_salt))
            .collect(),
        message_count: messages.len(),
        tool_count: tools.len(),
    }
}

fn remove_transport_fields(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.remove("stream");
        object.remove("stream_options");
    }
}

fn sha256_json(value: &Value, hash_salt: &[u8]) -> String {
    let canonical = canonical_json(value);
    let encoded = serde_json::to_vec(&canonical).expect("JSON values are always serializable");
    let mut hasher = Sha256::new();
    hasher.update(b"rara-model-request-fingerprint-v1\0");
    hasher.update(hash_salt);
    hasher.update(encoded);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        Value::Object(object) => {
            let sorted = object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(Map::from_iter(sorted))
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        MAX_COMPONENT_FINGERPRINTS, apply_deepseek_user_id, enable_streaming_usage,
        fingerprint_request,
    };
    use crate::config::OpenAiEndpointKind;

    #[test]
    fn deepseek_streaming_requests_include_usage() {
        let mut body = json!({"model": "deepseek-v4-flash"});

        enable_streaming_usage(&mut body, OpenAiEndpointKind::Deepseek);

        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn custom_streaming_requests_do_not_assume_usage_support() {
        let mut body = json!({"model": "custom"});

        enable_streaming_usage(&mut body, OpenAiEndpointKind::Custom);

        assert_eq!(body["stream"], true);
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn deepseek_user_id_is_not_sent_to_other_endpoint_kinds() {
        let mut deepseek = json!({});
        let mut custom = json!({});

        apply_deepseek_user_id(
            &mut deepseek,
            OpenAiEndpointKind::Deepseek,
            Some("cache-scope"),
        );
        apply_deepseek_user_id(&mut custom, OpenAiEndpointKind::Custom, Some("cache-scope"));

        assert_eq!(deepseek["user_id"], "cache-scope");
        assert!(custom.get("user_id").is_none());
    }

    #[test]
    fn fingerprint_is_canonical_and_contains_no_raw_content() {
        let left = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                {"role": "system", "content": "private system"},
                {"role": "user", "content": "private user"}
            ],
            "tools": [{"type": "function", "function": {"name": "private_tool"}}]
        });
        let right = json!({
            "tools": [{"function": {"name": "private_tool"}, "type": "function"}],
            "messages": [
                {"content": "private system", "role": "system"},
                {"content": "private user", "role": "user"}
            ],
            "model": "deepseek-v4-flash"
        });

        let left = fingerprint_request(&left, "scope", b"salt");
        let right = fingerprint_request(&right, "scope", b"salt");

        assert_eq!(left, right);
        let serialized = serde_json::to_string(&left).expect("serialize fingerprint");
        assert!(!serialized.contains("private"));
    }

    #[test]
    fn fingerprint_component_lists_are_bounded_but_preserve_total_counts() {
        let messages = (0..300)
            .map(|index| json!({"role": "user", "content": index.to_string()}))
            .collect::<Vec<_>>();
        let tools = (0..300)
            .map(|index| json!({"type": "function", "function": {"name": index.to_string()}}))
            .collect::<Vec<_>>();

        let fingerprint = fingerprint_request(
            &json!({"messages": messages, "tools": tools}),
            "scope",
            b"salt",
        );

        assert_eq!(fingerprint.message_count, 300);
        assert_eq!(fingerprint.tool_count, 300);
        assert_eq!(fingerprint.message_sha256.len(), MAX_COMPONENT_FINGERPRINTS);
        assert_eq!(fingerprint.tool_sha256.len(), MAX_COMPONENT_FINGERPRINTS);
    }

    #[test]
    fn transport_fields_do_not_change_the_logical_request_hash() {
        let base = json!({
            "model": "deepseek-v4-flash",
            "messages": [{"role": "system", "content": "stable"}]
        });
        let mut streaming = base.clone();
        enable_streaming_usage(&mut streaming, OpenAiEndpointKind::Deepseek);

        assert_eq!(
            fingerprint_request(&base, "scope", b"salt").request_sha256,
            fingerprint_request(&streaming, "scope", b"salt").request_sha256
        );
    }
}
