use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use rara_tools::tool::{Tool, ToolError, ToolManager};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub(super) struct Corpus {
    pub corpus_sha256: String,
    pub python_version: String,
    pub cases: Vec<Case>,
}

#[derive(Deserialize)]
pub(super) struct Case {
    pub case_id: u64,
    pub files: BTreeMap<String, String>,
    pub turns: Vec<Turn>,
}

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    Plan,
    Execute,
    Review,
}

#[derive(Deserialize)]
pub(super) struct Turn {
    pub mode: Mode,
    pub prompt: String,
    pub grade_phase: Option<u8>,
    #[serde(default)]
    pub compaction_boundary_after: bool,
}

pub(super) async fn python_receipt(python: &Path, arguments: &[String]) -> Result<Value> {
    let workspace = match std::env::var_os("TEST_SRCDIR") {
        Some(runfiles) => PathBuf::from(runfiles)
            .join(std::env::var_os("TEST_WORKSPACE").context("Bazel test workspace is required")?),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    };
    let script = workspace.join("tools/prefix_cache_eval/run.py");
    let mut command = tokio::process::Command::new(python);
    command.arg(script).args(arguments).kill_on_drop(true);
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(20), command.output()).await??;
    // Grade failure deliberately exits 1; its structured receipt is still useful.
    ensure!(
        output.status.success() || output.status.code() == Some(1),
        "fixture command failed"
    );
    ensure!(
        output.stdout.len() <= 100_000,
        "fixture receipt exceeds limit"
    );
    serde_json::from_slice(&output.stdout).context("fixture JSON receipt")
}

pub(super) fn initialize(case: &Case, root: &Path) -> Result<ToolManager> {
    ensure!(
        (1..=3).contains(&case.case_id) && !case.turns.is_empty() && case.turns.len() <= 4,
        "unsupported case"
    );
    std::fs::create_dir(root)?;
    for (name, content) in &case.files {
        ensure!(
            matches!(name.as_str(), "task.py" | "POLICY.md") && content.len() <= 64_000,
            "unsupported fixture file"
        );
        std::fs::write(root.join(name), content)?;
    }
    ensure!(root.join("task.py").is_file(), "task.py is required");
    let root = root.canonicalize()?;
    let mut tools = ToolManager::new();
    for inner in [
        Box::<rara_tools::file::ReadFileTool>::default() as Box<dyn Tool>,
        Box::<rara_tools::file::WriteFileTool>::default(),
    ] {
        tools.register(Box::new(FixtureTool {
            inner,
            root: root.clone(),
            files: case.files.keys().cloned().collect(),
        }));
    }
    Ok(tools)
}

// Delegate the real file schemas and behavior, restricting access to the seeded
// fixture files. No shell, extensions, child tools, or verifier source is exposed.
struct FixtureTool {
    inner: Box<dyn Tool>,
    root: PathBuf,
    files: Vec<String>,
}

#[async_trait::async_trait]
impl Tool for FixtureTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }
    async fn call(&self, mut input: Value) -> Result<Value, ToolError> {
        let path = input["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("path".into()))?;
        let path = self.root.join(path).canonicalize()?;
        if !self.files.iter().any(|file| path == self.root.join(file)) {
            return Err(ToolError::InvalidInput(
                "path is outside the prepared fixture".into(),
            ));
        }
        if input
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|text| text.len() > 64_000)
        {
            return Err(ToolError::InvalidInput(
                "fixture write exceeds limit".into(),
            ));
        }
        input["path"] = json!(path);
        self.inner.call(input).await
    }
}

pub(super) fn snapshot(case: &Case, root: &Path) -> Result<Vec<Vec<u8>>> {
    case.files
        .keys()
        .map(|name| std::fs::read(root.join(name)).map_err(Into::into))
        .collect()
}
