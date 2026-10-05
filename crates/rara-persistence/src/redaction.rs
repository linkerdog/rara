use std::sync::LazyLock;

use regex::Regex;

const REDACTED_SECRET: &str = "[REDACTED_SECRET]";
const REDACTED_URL_VALUE: &str = "<redacted>";
const SENSITIVE_URL_QUERY_KEYS: &[&str] = &[
    "access_token",
    "api_key",
    "client_secret",
    "code",
    "code_verifier",
    "exaApiKey",
    "id_token",
    "key",
    "refresh_token",
    "requested_token",
    "state",
    "subject_token",
    "token",
];

static OPENAI_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"sk-[A-Za-z0-9_-]{20,}"));
static PROVIDER_TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|AIza[0-9A-Za-z_-]{35})",
    )
});
static AWS_ACCESS_KEY_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bAKIA[0-9A-Z]{16}\b"));
static BEARER_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"(?i)\bBearer\s+[A-Za-z0-9._\-]{16,}\b"));
static SECRET_ASSIGNMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // `_` is a word character, so a bare `\btoken\b` never fires inside `GITHUB_TOKEN=`.
    // Allow an identifier prefix and an `_`-continued suffix (`AWS_SECRET_ACCESS_KEY`), but not
    // a bare suffix such as `tokenizer`.
    compile_regex(
        r#"(?i)\b([a-z0-9_.-]*(?:api[_-]?key|access[_-]?key|private[_-]?key|token|secret|passwd|password)(?:_[a-z0-9_]*)?)\b(\s*[:=]\s*)(["']?)[^\s"']{8,}"#,
    )
});
static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| compile_regex(r#"https?://[^\s)>"]+"#));

pub fn redact_secrets(input: impl Into<String>) -> String {
    let input = input.into();
    let redacted = OPENAI_KEY_REGEX.replace_all(&input, REDACTED_SECRET);
    let redacted = PROVIDER_TOKEN_REGEX.replace_all(&redacted, REDACTED_SECRET);
    let redacted = AWS_ACCESS_KEY_ID_REGEX.replace_all(&redacted, REDACTED_SECRET);
    let redacted = BEARER_TOKEN_REGEX.replace_all(&redacted, "Bearer [REDACTED_SECRET]");
    let redacted = SECRET_ASSIGNMENT_REGEX.replace_all(&redacted, "$1$2$3[REDACTED_SECRET]");
    redact_urls(&redacted)
}

pub fn sanitize_url_for_display(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_fragment(None);

            let query_pairs = url
                .query_pairs()
                .map(|(key, value)| {
                    let key = key.into_owned();
                    let value = value.into_owned();
                    if SENSITIVE_URL_QUERY_KEYS
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(&key))
                    {
                        (key, REDACTED_URL_VALUE.to_string())
                    } else {
                        (key, value)
                    }
                })
                .collect::<Vec<_>>();

            if query_pairs.is_empty() {
                url.set_query(None);
            } else {
                let redacted_query = query_pairs
                    .into_iter()
                    .fold(
                        url::form_urlencoded::Serializer::new(String::new()),
                        |mut serializer, (key, value)| {
                            serializer.append_pair(&key, &value);
                            serializer
                        },
                    )
                    .finish();
                url.set_query(Some(&redacted_query));
            }

            url.to_string()
        }
        Err(_) => "<invalid-url>".to_string(),
    }
}

fn redact_urls(input: &str) -> String {
    URL_REGEX
        .replace_all(input, |captures: &regex::Captures<'_>| {
            sanitize_url_for_display(&captures[0])
        })
        .to_string()
}

fn compile_regex(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|err| panic!("invalid regex pattern '{pattern}': {err}"))
}

#[cfg(test)]
mod tests {
    use super::{redact_secrets, sanitize_url_for_display};

    #[test]
    fn redacts_bearer_tokens_and_assignments() {
        let rendered = redact_secrets(
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz token=supersecretvalue".to_string(),
        );
        assert!(!rendered.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!rendered.contains("supersecretvalue"));
        assert!(rendered.contains("[REDACTED_SECRET]"));
    }

    #[test]
    fn redacts_env_style_assignments() {
        for line in [
            "export GITHUB_TOKEN=ghx_notarealvalue123",
            "OPENAI_API_KEY=abcdef123456",
            "AWS_SECRET_ACCESS_KEY=abcdef123456",
            "DB_PASSWORD='hunter2hunter2'",
            "my.api-key: abcdef123456",
        ] {
            let rendered = redact_secrets(line.to_string());
            assert!(
                rendered.contains("[REDACTED_SECRET]"),
                "not redacted: {line}"
            );
            assert!(!rendered.contains("abcdef123456"), "leaked: {line}");
            assert!(!rendered.contains("hunter2hunter2"), "leaked: {line}");
            assert!(!rendered.contains("notarealvalue"), "leaked: {line}");
        }
    }

    #[test]
    fn redacts_prefixed_provider_tokens() {
        for secret in [
            "sk-proj-abcdefghij_klmnopqrstuv",
            "sk-ant-api03-abcdefghijklmnopqrstuv",
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "github_pat_abcdefghijklmnopqrstuv_0123",
            "xoxb-1234567890-abcdefghij",
            "AIzaSyA-abcdefghijklmnopqrstuvwxyz012345",
        ] {
            let rendered = redact_secrets(format!("use {secret} please"));
            assert!(!rendered.contains(secret), "leaked: {secret}");
        }
    }

    #[test]
    fn keeps_short_or_unrelated_assignments() {
        for line in [
            "max_tokens=4096",
            "name=value_that_is_long",
            "mode: production",
        ] {
            assert_eq!(redact_secrets(line.to_string()), line);
        }
    }

    #[test]
    fn sanitizes_sensitive_url_parts() {
        let rendered = sanitize_url_for_display(
            "https://user:pass@example.com/path?token=abc123&env=prod#fragment",
        );
        assert_eq!(
            rendered,
            "https://example.com/path?token=%3Credacted%3E&env=prod".to_string()
        );
    }

    #[test]
    fn sanitizes_exa_api_key_query_param() {
        let rendered =
            sanitize_url_for_display("https://mcp.exa.ai/mcp?exaApiKey=secret&mode=search");

        assert_eq!(
            rendered,
            "https://mcp.exa.ai/mcp?exaApiKey=%3Credacted%3E&mode=search".to_string()
        );
    }
}
