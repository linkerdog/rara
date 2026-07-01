use serde_json::{Map, Value};

/// Whitelist of keys that Gemini's `Schema` object accepts.
/// Gemini rejects `additionalProperties`, `$schema`, `$defs`, etc.
/// See: <https://ai.google.dev/api/generate-content#Schema>
const GEMINI_SCHEMA_ALLOWED_KEYS: &[&str] = &[
    "type",
    "format",
    "title",
    "description",
    "nullable",
    "enum",
    "maxItems",
    "minItems",
    "properties",
    "required",
    "minProperties",
    "maxProperties",
    "minLength",
    "maxLength",
    "pattern",
    "example",
    "anyOf",
    "propertyOrdering",
    "default",
    "items",
    "minimum",
    "maximum",
];

/// Returns a Gemini-compatible copy of a tool parameter schema.
///
/// RARA's tool schemas are OpenAI-flavored JSON Schema and may contain keys
/// such as `$schema` or `additionalProperties` that Gemini's `Schema` object
/// rejects. This helper preserves only the documented Gemini subset and
/// recursively sanitizes nested `properties`, `items`, and `anyOf` definitions.
pub fn sanitize_gemini_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut cleaned = Map::new();
            for (key, value) in map {
                if !GEMINI_SCHEMA_ALLOWED_KEYS.contains(&key.as_str()) {
                    continue;
                }
                match key.as_str() {
                    "properties" => {
                        if let Value::Object(props) = value {
                            let mut cleaned_props = Map::new();
                            for (prop_name, prop_schema) in props {
                                cleaned_props
                                    .insert(prop_name.clone(), sanitize_gemini_schema(prop_schema));
                            }
                            cleaned.insert(key.clone(), Value::Object(cleaned_props));
                        }
                    }
                    "items" => {
                        cleaned.insert(key.clone(), sanitize_gemini_schema(value));
                    }
                    "anyOf" => {
                        if let Value::Array(schemas) = value {
                            let cleaned_anyof: Vec<Value> = schemas
                                .iter()
                                .filter(|v| v.is_object())
                                .map(sanitize_gemini_schema)
                                .collect();
                            cleaned.insert(key.clone(), Value::Array(cleaned_anyof));
                        }
                    }
                    _ => {
                        cleaned.insert(key.clone(), value.clone());
                    }
                }
            }

            // Gemini's Schema validator requires every `enum` entry to be a
            // string, even when the parent `type` is `integer` / `number` /
            // `boolean`. We drop the `enum` when it would violate Gemini's rule
            // — keeping `type` plus the description gives the model enough
            // guidance; the tool handler still validates the value.
            if let Some(enum_val) = cleaned.get("enum") {
                let type_val = cleaned.get("type").and_then(|v| v.as_str());
                if matches!(type_val, Some("integer" | "number" | "boolean"))
                    && let Value::Array(items) = enum_val
                    && items.iter().any(|v| !v.is_string())
                {
                    cleaned.remove("enum");
                }
            }

            Value::Object(cleaned)
        }
        _ => Value::Null,
    }
}

/// Normalize tool parameters to a valid Gemini object schema.
///
/// If the input is empty or invalid, returns a minimal `{type: "object",
/// properties: {}}` schema.
#[cfg(test)]
pub fn sanitize_gemini_tool_parameters(parameters: &Value) -> Value {
    let cleaned = sanitize_gemini_schema(parameters);
    if cleaned.is_null() || cleaned.as_object().is_none_or(|o| o.is_empty()) {
        serde_json::json!({"type": "object", "properties": {}})
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn removes_additional_properties() {
        let schema = json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        });
        let cleaned = sanitize_gemini_schema(&schema);
        assert!(cleaned.get("additionalProperties").is_none());
    }

    #[test]
    fn removes_dollar_schema() {
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {}
        });
        let cleaned = sanitize_gemini_schema(&schema);
        assert!(cleaned.get("$schema").is_none());
        assert_eq!(cleaned.get("type").and_then(|v| v.as_str()), Some("object"));
    }

    #[test]
    fn drops_non_string_enum_for_integer_type() {
        let schema = json!({
            "type": "integer",
            "enum": [60, 1440, 4320],
            "description": "archive duration in minutes"
        });
        let cleaned = sanitize_gemini_schema(&schema);
        assert!(cleaned.get("enum").is_none());
        assert_eq!(
            cleaned.get("type").and_then(|v| v.as_str()),
            Some("integer")
        );
        assert_eq!(
            cleaned.get("description").and_then(|v| v.as_str()),
            Some("archive duration in minutes")
        );
    }

    #[test]
    fn keeps_string_enum() {
        let schema = json!({
            "type": "string",
            "enum": ["celsius", "fahrenheit"]
        });
        let cleaned = sanitize_gemini_schema(&schema);
        assert_eq!(cleaned.get("enum").unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn recursively_cleans_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "additionalProperties": false
                }
            }
        });
        let cleaned = sanitize_gemini_schema(&schema);
        let props = cleaned.get("properties").unwrap();
        let name = props.get("name").unwrap();
        assert!(name.get("additionalProperties").is_none());
        assert_eq!(name.get("type").and_then(|v| v.as_str()), Some("string"));
    }

    #[test]
    fn empty_or_invalid_returns_minimal_object() {
        assert_eq!(
            sanitize_gemini_tool_parameters(&json!(null)),
            json!({"type": "object", "properties": {}})
        );
        assert_eq!(
            sanitize_gemini_tool_parameters(&json!({})),
            json!({"type": "object", "properties": {}})
        );
    }

    #[test]
    fn cleans_nested_anyof() {
        let schema = json!({
            "type": "object",
            "properties": {
                "mode": {
                    "anyOf": [
                        {
                            "type": "string",
                            "enum": ["auto", "manual"],
                            "additionalProperties": false
                        },
                        {
                            "type": "null"
                        }
                    ]
                }
            }
        });
        let cleaned = sanitize_gemini_schema(&schema);
        let props = cleaned.get("properties").unwrap();
        let mode = props.get("mode").unwrap();
        let anyof = mode.get("anyOf").unwrap().as_array().unwrap();
        assert_eq!(anyof.len(), 2);
        assert!(anyof[0].get("additionalProperties").is_none());
    }
}
