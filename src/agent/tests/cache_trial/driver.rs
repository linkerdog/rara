//! Opt-in paid experiment driver; ordinary tests use deterministic backends.

mod budget;
mod fixtures;
mod tests;

use std::fs::OpenOptions;
use std::io::Write;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use rara_observability::{
    InferenceExperimentArm, InferenceExperimentSample, InferencePriceTable, InferenceTask,
};
use serde_json::{Value, json};

use self::budget::{Budget, CappedBackend, TariffWindow};
use self::fixtures::{Case, Corpus, Mode, initialize, python_receipt, snapshot};
use crate::agent::{AgentExecutionMode, AgentOutputMode, CacheExperimentOptions, ToolSchemaPolicy};
use crate::config::{OpenAiEndpointKind, RaraConfig};
use crate::llm::{LlmBackend, OpenAiCompatibleBackend, SummaryStrategy};
use crate::runtime_context::{
    RuntimeBootstrapOptions, initialize_rara_context_for_workspace_with_options,
};

#[derive(Clone, Copy, PartialEq)]
enum Comparison {
    Tools,
    Summary,
    Compaction,
}

#[derive(serde::Deserialize)]
struct PaidTariff {
    table: InferencePriceTable,
    validity: TariffWindow,
}

impl Comparison {
    fn options(self, arm: InferenceExperimentArm) -> CacheExperimentOptions {
        let candidate = arm == InferenceExperimentArm::Candidate;
        CacheExperimentOptions {
            tool_schemas: if self == Self::Tools && candidate {
                ToolSchemaPolicy::SessionStable
            } else {
                ToolSchemaPolicy::ModeFiltered
            },
            summary: if self == Self::Summary && candidate {
                SummaryStrategy::CachedMainModel
            } else {
                SummaryStrategy::AuxiliaryModel
            },
        }
    }
    fn compact(self, arm: InferenceExperimentArm) -> bool {
        self == Self::Summary
            || (self == Self::Compaction && arm == InferenceExperimentArm::Candidate)
    }
}

struct CaseRun<'a> {
    case: &'a Case,
    corpus_sha256: &'a str,
    root: &'a Path,
    python: &'a Path,
    backend: Arc<dyn LlmBackend>,
    accounting: InferenceTask,
    arm: InferenceExperimentArm,
    comparison: Comparison,
    repetition: u32,
}

async fn run_case(run: CaseRun<'_>) -> Result<(InferenceExperimentSample, Value)> {
    let workspace = run.root.join("workspace");
    std::fs::create_dir(run.root)?;
    let tools = initialize(run.case, &workspace)?;
    let mut config = RaraConfig::default();
    config.set_provider("mock");
    let options = RuntimeBootstrapOptions::default()
        .with_rara_home(Some(run.root.join("state")))
        .with_backend(Some(run.backend))
        .with_tool_manager(Some(tools))
        .with_extension_discovery(false)
        .with_memory_facilities(false)
        .with_transcript_persistence(false);
    let bootstrap = initialize_rara_context_for_workspace_with_options(
        &config,
        Some(&workspace),
        None,
        options,
    )
    .await?;
    let mut agent = bootstrap.into_agent().await;
    // Ignored provider tests must not inherit the 10 ms unit-test deadline.
    agent.compact_state.use_production_summary_timeout();
    agent.set_max_turns(6);
    agent.configure_cache_experiment(run.comparison.options(run.arm));
    let started = Instant::now();
    let started_unix_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let mut passed = Some(true);
    let mut grades = Vec::new();
    let mut model_turns = Vec::new();
    let mut compactions = 0;
    let mut failure_stage = None;
    for turn in &run.case.turns {
        agent.execution_mode = match turn.mode {
            Mode::Plan => AgentExecutionMode::Plan,
            Mode::Execute => AgentExecutionMode::Execute,
            Mode::Review => AgentExecutionMode::Review,
        };
        let before = match snapshot(run.case, &workspace) {
            Ok(before) => before,
            Err(_) => {
                passed = None;
                failure_stage = Some("workspace_read");
                break;
            }
        };
        agent.pending_inference_agent = Some(run.accounting.start_agent(None));
        let result = agent
            .query_with_mode(turn.prompt.clone(), AgentOutputMode::Silent)
            .await;
        model_turns.extend(agent.last_query_report.model_turns.clone());
        if result.is_err() {
            // An interrupted operation has no externally graded outcome.
            passed = None;
            failure_stage = Some("model_call");
            break;
        }
        if turn.mode != Mode::Execute {
            match snapshot(run.case, &workspace) {
                Ok(after) if after != before => {
                    passed = Some(false);
                    failure_stage = Some("mode_constraint");
                }
                Ok(_) => {}
                Err(_) => {
                    passed = None;
                    failure_stage = Some("workspace_read");
                    break;
                }
            }
        }
        if let Some(phase) = turn.grade_phase {
            let receipt = python_receipt(
                run.python,
                &[
                    "grade".into(),
                    "--case".into(),
                    run.case.case_id.to_string(),
                    "--phase".into(),
                    phase.to_string(),
                    "--workspace".into(),
                    workspace.display().to_string(),
                ],
            )
            .await;
            let receipt = match receipt {
                Ok(receipt) if receipt["corpus_sha256"] == run.corpus_sha256 => receipt,
                Ok(_) | Err(_) => {
                    passed = None;
                    failure_stage = Some("grader_receipt");
                    break;
                }
            };
            let grade = receipt["passed"].as_bool();
            passed = match (passed, grade) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            };
            grades.push(receipt);
        }
        if turn.compaction_boundary_after && run.comparison.compact(run.arm) {
            let lease = run.accounting.start_agent(None);
            agent.inference_context = Some(lease.context());
            let compacted = agent.compact_now_with_reporter(|_| {}).await;
            drop(lease);
            if !matches!(compacted, Ok(true)) {
                passed = None;
                failure_stage = Some("compaction");
                break;
            }
            compactions += 1;
        }
    }
    if grades.len()
        != run
            .case
            .turns
            .iter()
            .filter(|turn| turn.grade_phase.is_some())
            .count()
    {
        passed = None;
    }
    if let Some(control) = agent.agent_tree_control()
        && control.shutdown().await.is_err()
    {
        passed = None;
        failure_stage = Some("shutdown");
    }
    let accounting = run.accounting.snapshot();
    if !accounting.is_terminal() {
        passed = None;
        failure_stage = Some("undrained_work");
    }
    let sample = InferenceExperimentSample {
        case_id: run.case.case_id,
        repetition: run.repetition,
        arm: run.arm,
        passed,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        accounting,
    };
    Ok((
        sample,
        json!({"started_unix_ms":started_unix_ms, "grades":grades, "compactions":compactions, "failure_stage":failure_stage, "model_turns":model_turns}),
    ))
}

/// Run only after selecting the actual tariff window and authorizing its ceiling.
/// The process must run in the task execution sandbox described by the corpus.
#[tokio::test]
#[ignore = "requires explicit paid-call acknowledgement, tariff, budget, and isolated execution"]
async fn run_paid_prefix_cache_comparison() -> Result<()> {
    ensure!(
        std::env::var("CACHE_TRIAL_ALLOW_PAID").as_deref() == Ok("yes"),
        "set CACHE_TRIAL_ALLOW_PAID=yes explicitly"
    );
    let limit_usd: f64 = std::env::var("CACHE_TRIAL_MAX_USD")
        .context("CACHE_TRIAL_MAX_USD is required")?
        .parse()?;
    let tariff: PaidTariff = serde_json::from_slice(&std::fs::read(
        std::env::var("CACHE_TRIAL_PRICES").context("CACHE_TRIAL_PRICES is required")?,
    )?)?;
    let prices = tariff.table;
    let comparison_name =
        std::env::var("CACHE_TRIAL_COMPARISON").context("select tools, summary, or compaction")?;
    let comparison = match comparison_name.as_str() {
        "tools" => Comparison::Tools,
        "summary" => Comparison::Summary,
        "compaction" => Comparison::Compaction,
        _ => anyhow::bail!("select tools, summary, or compaction"),
    };
    let repetitions: u32 = std::env::var("CACHE_TRIAL_REPETITIONS")
        .unwrap_or_else(|_| "2".into())
        .parse()?;
    ensure!(
        (1..=3).contains(&repetitions),
        "repetitions must be in 1..=3"
    );
    let python =
        PathBuf::from(std::env::var("CACHE_TRIAL_PYTHON").unwrap_or_else(|_| "python3".into()));
    let corpus: Corpus =
        serde_json::from_value(python_receipt(&python, &["export".into()]).await?)?;
    let config_path = crate::config::rara_home_dir()?.join("config.json");
    let config = crate::config::ConfigManager { path: config_path }.load()?;
    ensure!(
        config.active_openai_profile_kind() == Some(OpenAiEndpointKind::Deepseek),
        "the selected config must be the DeepSeek profile"
    );
    let surface = config.effective_provider_surface();
    let base_url = surface.base_url.value.unwrap_or("https://api.deepseek.com");
    ensure!(
        matches!(
            base_url.trim_end_matches('/'),
            "https://api.deepseek.com" | "https://api.deepseek.com/v1"
        ),
        "the selected profile must use the official DeepSeek endpoint"
    );
    let model = config.model.clone().context("configured main model")?;
    let auxiliary = config
        .auxiliary_model
        .clone()
        .or_else(|| {
            crate::llm::infer_openai_compatible_auxiliary_model(
                &model,
                OpenAiEndpointKind::Deepseek,
            )
            .map(std::borrow::Cow::into_owned)
        })
        .unwrap_or_else(|| model.clone());
    for selected in [&model, &auxiliary] {
        ensure!(
            matches!(
                selected.as_str(),
                "deepseek-v4-pro" | "deepseek-v4-flash" | "deepseek-flash"
            ),
            "trial supports verified DeepSeek V4 endpoints only"
        );
        ensure!(
            prices
                .prices
                .iter()
                .filter(|price| price.provider == "DeepSeek" && &price.model == selected)
                .count()
                == 1,
            "supply one exact tariff for each selected model"
        );
    }
    let key = config
        .api_key_secret()
        .context("configured API credential")?
        .clone();
    let max_output = NonZeroU32::new(4096).expect("positive limit");
    let budget = Arc::new(Mutex::new(
        Budget::new(limit_usd, &prices, max_output.get())?.with_tariff_window(tariff.validity)?,
    ));
    let max_call_usd = budget.lock().expect("trial budget").reserved_call_usd();
    ensure!(
        limit_usd >= max_call_usd,
        "ceiling must cover one worst-case logical call ({max_call_usd:.6} USD); unused reservation is refunded from complete receipts"
    );
    let output_path =
        std::env::var("CACHE_TRIAL_OUTPUT").context("CACHE_TRIAL_OUTPUT is required")?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)?;
    let run_root = tempfile::tempdir()?.keep();
    let started_unix_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    writeln!(
        output,
        "{}",
        json!({"kind":"run", "corpus_sha256":corpus.corpus_sha256, "python_version":corpus.python_version, "started_unix_ms":started_unix_ms, "endpoint":base_url, "model":model, "auxiliary_model":auxiliary, "comparison":comparison_name, "repetitions":repetitions, "max_output_tokens":max_output, "max_model_turns":6, "max_usd":limit_usd, "max_reserved_call_usd":max_call_usd, "tariff_validity":tariff.validity, "prices":prices})
    )?;
    output.flush()?;
    let mut samples = Vec::new();
    'trials: for repetition in 0..repetitions {
        for case in &corpus.cases {
            if comparison != Comparison::Tools
                && !case.turns.iter().any(|turn| turn.compaction_boundary_after)
            {
                continue;
            }
            let arms = if repetition % 2 == 0 {
                [
                    InferenceExperimentArm::Baseline,
                    InferenceExperimentArm::Candidate,
                ]
            } else {
                [
                    InferenceExperimentArm::Candidate,
                    InferenceExperimentArm::Baseline,
                ]
            };
            for arm in arms {
                let accounting = InferenceTask::default();
                let inner = OpenAiCompatibleBackend::new_with_endpoint_kind_and_reasoning(
                    Some(key.clone()),
                    base_url.into(),
                    model.clone(),
                    OpenAiEndpointKind::Deepseek,
                    config.reasoning_effort.clone(),
                    config.thinking,
                )?
                .with_auxiliary_model(Some(auxiliary.clone()))
                .with_max_output_tokens(max_output)
                .with_deepseek_user_id(uuid::Uuid::new_v4().to_string());
                let backend = Arc::new(CappedBackend {
                    inner: Arc::new(inner),
                    task: accounting.clone(),
                    prices: prices.clone(),
                    budget: budget.clone(),
                });
                let root = run_root.join(format!("{}-{repetition}-{arm:?}", case.case_id));
                let (sample, detail) = run_case(CaseRun {
                    case,
                    corpus_sha256: &corpus.corpus_sha256,
                    root: &root,
                    python: &python,
                    backend,
                    accounting,
                    arm,
                    comparison,
                    repetition,
                })
                .await?;
                let complete = prices.cost(&sample.accounting).complete;
                writeln!(
                    output,
                    "{}",
                    json!({"kind":"sample", "sample":sample, "detail":detail})
                )?;
                output.flush()?;
                let graded = sample.passed.is_some();
                samples.push(sample);
                if !complete || !graded {
                    break 'trials;
                }
            }
        }
    }
    writeln!(
        output,
        "{}",
        json!({"kind":"comparison", "report":prices.compare_tasks(&samples)})
    )?;
    output.flush()?;
    eprintln!(
        "Trial fixture workspaces retained at {}",
        run_root.display()
    );
    Ok(())
}
