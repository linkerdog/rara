use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

pub(super) fn parse_document(content: &str) -> Result<Value> {
    let mut bytes = content.as_bytes().to_vec();
    let mut index = 0;
    let mut quoted = false;
    while index < bytes.len() {
        if quoted {
            match bytes[index] {
                b'\\' => index += 1,
                b'"' => quoted = false,
                _ => {}
            }
        } else if bytes[index] == b'"' {
            quoted = true;
        } else if bytes[index..].starts_with(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                bytes[index] = b' ';
                index += 1;
            }
            continue;
        } else if bytes[index..].starts_with(b"/*") {
            bytes[index] = b' ';
            bytes[index + 1] = b' ';
            index += 2;
            while index < bytes.len() && !bytes[index..].starts_with(b"*/") {
                if bytes[index] != b'\n' && bytes[index] != b'\r' {
                    bytes[index] = b' ';
                }
                index += 1;
            }
            if index + 1 >= bytes.len() {
                bail!("Unterminated JSONC comment");
            }
            bytes[index] = b' ';
            bytes[index + 1] = b' ';
            index += 2;
            continue;
        }
        index += 1;
    }
    // Remove trailing commas only outside strings, after comments are blanked.
    index = 0;
    quoted = false;
    while index < bytes.len() {
        if quoted {
            match bytes[index] {
                b'\\' => index += 1,
                b'"' => quoted = false,
                _ => {}
            }
        } else if bytes[index] == b'"' {
            quoted = true;
        } else if bytes[index] == b','
            && bytes[index + 1..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| matches!(b, b'}' | b']'))
        {
            bytes[index] = b' ';
        }
        index += 1;
    }
    // Syntax diagnostics contain positions, never the potentially secret document.
    serde_json::from_slice(&bytes).map_err(|err| {
        anyhow::anyhow!(
            "Invalid provider JSON at line {}, column {}",
            err.line(),
            err.column()
        )
    })
}

pub(super) fn merge(target: &mut Value, source: Value) {
    if let (Value::Object(target), Value::Object(source)) = (&mut *target, &source) {
        for (key, value) in source {
            merge(
                target.entry(key.clone()).or_insert(Value::Null),
                value.clone(),
            );
        }
    } else {
        *target = source;
    }
}

pub(super) fn expand(
    value: &mut Value,
    directory: &Path,
    read_env: &impl Fn(&str) -> Option<String>,
) -> Result<()> {
    match value {
        Value::String(text) => {
            let mut result = String::new();
            let mut remaining = text.as_str();
            while let Some(start) = remaining.find('{') {
                result.push_str(&remaining[..start]);
                remaining = &remaining[start..];
                let prefix = if remaining.starts_with("{env:") {
                    "{env:"
                } else if remaining.starts_with("{file:") {
                    "{file:"
                } else {
                    result.push('{');
                    remaining = &remaining[1..];
                    continue;
                };
                let Some(end) = remaining.find('}') else {
                    break;
                };
                let key = &remaining[prefix.len()..end];
                let replacement = if prefix == "{env:" {
                    read_env(key).unwrap_or_default()
                } else {
                    let path = if let Some(suffix) = key.strip_prefix("~/") {
                        std::path::PathBuf::from(
                            read_env("HOME").context("HOME is required for ~/ file references")?,
                        )
                        .join(suffix)
                    } else {
                        directory.join(key)
                    };
                    std::fs::read_to_string(&path)
                        .with_context(|| {
                            format!("Cannot read provider variable file {}", path.display())
                        })?
                        .trim()
                        .to_string()
                };
                result.push_str(&replacement);
                remaining = &remaining[end + 1..];
            }
            result.push_str(remaining);
            *text = result;
        }
        Value::Array(values) => {
            for value in values {
                expand(value, directory, read_env)?;
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                expand(value, directory, read_env)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}
