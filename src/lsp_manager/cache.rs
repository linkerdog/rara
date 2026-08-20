use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::types::{Diagnostic, DiagnosticFreshness, DiagnosticSeverity, LspDiagnostics};

#[derive(Debug, Clone)]
struct CachedDiagnosticSet {
    document_version: i64,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Default)]
pub(super) struct DiagnosticCache {
    expected_versions: HashMap<PathBuf, i64>,
    entries: HashMap<PathBuf, CachedDiagnosticSet>,
}

impl DiagnosticCache {
    pub(super) fn mark_expected_version(&mut self, path: PathBuf, version: i64) {
        self.expected_versions.insert(path, version);
    }

    pub(super) fn publish(
        &mut self,
        path: PathBuf,
        published_version: Option<i64>,
        diagnostics: Vec<Diagnostic>,
    ) -> bool {
        let expected_version = self.expected_versions.get(&path).copied().unwrap_or(0);
        if published_version.is_some_and(|version| version < expected_version) {
            return false;
        }
        let document_version = published_version.unwrap_or(expected_version);
        if self
            .entries
            .get(&path)
            .is_some_and(|entry| entry.document_version > document_version)
        {
            return false;
        }
        self.entries.insert(
            path,
            CachedDiagnosticSet {
                document_version,
                diagnostics,
            },
        );
        true
    }

    pub(super) fn get(&self, path: &Path) -> LspDiagnostics {
        let Some(entry) = self.entries.get(path) else {
            return LspDiagnostics {
                diagnostics: Vec::new(),
                freshness: DiagnosticFreshness::Pending,
            };
        };
        let expected_version = self.expected_versions.get(path).copied().unwrap_or(0);
        let freshness = if entry.document_version >= expected_version {
            DiagnosticFreshness::Current
        } else {
            DiagnosticFreshness::Cached
        };
        LspDiagnostics {
            diagnostics: entry.diagnostics.clone(),
            freshness,
        }
    }

    pub(super) fn counts(&self) -> (usize, usize) {
        (
            self.entries.len(),
            self.entries
                .values()
                .map(|entry| entry.diagnostics.len())
                .sum(),
        )
    }

    pub(super) fn summary(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let mut paths = self.entries.keys().collect::<Vec<_>>();
        paths.sort();
        let mut lines = vec!["# LSP Diagnostics".to_string()];
        for path in paths {
            let Some(entry) = self.entries.get(path) else {
                continue;
            };
            for diagnostic in &entry.diagnostics {
                let severity = match diagnostic.severity {
                    DiagnosticSeverity::Error => "error",
                    DiagnosticSeverity::Warning => "warning",
                    DiagnosticSeverity::Information => "info",
                    DiagnosticSeverity::Hint => "hint",
                };
                let code = diagnostic
                    .code
                    .as_ref()
                    .map(|code| format!("[{code}]"))
                    .unwrap_or_default();
                lines.push(format!(
                    "  {}:{}:{} {}{} {}",
                    diagnostic.file,
                    diagnostic.line + 1,
                    diagnostic.column + 1,
                    severity,
                    code,
                    diagnostic.message
                ));
            }
        }
        lines.join("\n")
    }
}

pub(super) fn parse_publish_diagnostics(
    params: Option<Value>,
    workspace_root: &Path,
) -> Option<(PathBuf, Option<i64>, Vec<Diagnostic>)> {
    let params = params?;
    let uri = params.get("uri")?.as_str()?;
    let path = path_from_file_uri(uri)?;
    let version = params.get("version").and_then(Value::as_i64);
    let diagnostics = params.get("diagnostics")?.as_array()?;
    let file = display_path_for_diagnostic(&path, workspace_root);
    let parsed = diagnostics
        .iter()
        .filter_map(|item| parse_diagnostic(item, &file))
        .collect::<Vec<_>>();
    Some((path, version, parsed))
}

fn parse_diagnostic(value: &Value, file: &str) -> Option<Diagnostic> {
    let start = value.get("range")?.get("start")?;
    let line = u32::try_from(start.get("line")?.as_u64()?).ok()?;
    let column = u32::try_from(start.get("character")?.as_u64()?).ok()?;
    let severity = match value.get("severity").and_then(Value::as_u64) {
        Some(1) => DiagnosticSeverity::Error,
        Some(2) => DiagnosticSeverity::Warning,
        Some(3) => DiagnosticSeverity::Information,
        Some(4) | None => DiagnosticSeverity::Hint,
        Some(_) => DiagnosticSeverity::Information,
    };
    let message = value.get("message")?.as_str()?.to_string();
    let code = value.get("code").and_then(|code| match code {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    });
    Some(Diagnostic {
        file: file.to_string(),
        line,
        column,
        severity,
        message,
        code,
    })
}

fn path_from_file_uri(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

fn display_path_for_diagnostic(path: &Path, workspace_root: &Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_publish_diagnostics_notification() {
        let workspace = PathBuf::from("/repo");
        let params = serde_json::json!({
            "uri": "file:///repo/src/main.rs",
            "version": 3,
            "diagnostics": [{
                "range": { "start": { "line": 4, "character": 8 }, "end": { "line": 4, "character": 12 } },
                "severity": 1,
                "message": "cannot find value `x` in this scope",
                "code": "E0425"
            }]
        });

        let (path, version, diagnostics) =
            parse_publish_diagnostics(Some(params), &workspace).expect("diagnostics");

        assert_eq!(path, PathBuf::from("/repo/src/main.rs"));
        assert_eq!(version, Some(3));
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostics[0].code.as_deref(), Some("E0425"));
    }

    #[test]
    fn stale_publication_does_not_replace_current_cache() {
        let path = PathBuf::from("/repo/src/main.rs");
        let mut cache = DiagnosticCache::default();
        cache.mark_expected_version(path.clone(), 2);

        assert!(!cache.publish(path.clone(), Some(1), Vec::new()));
        assert_eq!(cache.get(&path).freshness, DiagnosticFreshness::Pending);
        assert!(cache.publish(path.clone(), Some(2), Vec::new()));
        assert_eq!(cache.get(&path).freshness, DiagnosticFreshness::Current);

        cache.mark_expected_version(path.clone(), 3);
        assert_eq!(cache.get(&path).freshness, DiagnosticFreshness::Cached);
    }
}
