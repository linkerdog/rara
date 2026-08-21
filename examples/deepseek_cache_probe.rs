use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use rara::{
    DEFAULT_DEEPSEEK_CACHE_PROBE_MODEL, DeepseekCacheProbeOptions, RaraConfig,
    run_deepseek_cache_probe,
};
use uuid::Uuid;

#[derive(Parser)]
#[command(about = "Run an opt-in paired DeepSeek prefix-cache measurement")]
struct Cli {
    /// Perform paid network requests. Without this flag the command is a dry run.
    #[arg(long)]
    live: bool,

    /// Confirm that the requested model calls may incur DeepSeek API charges.
    #[arg(long)]
    acknowledge_cost: bool,

    #[arg(long, default_value = DEFAULT_DEEPSEEK_CACHE_PROBE_MODEL)]
    model: String,

    #[arg(long, default_value_t = 3)]
    pairs: usize,

    #[arg(long, default_value_t = 3)]
    turns_per_arm: usize,

    #[arg(long, default_value_t = 64)]
    max_output_tokens: u32,

    #[arg(long, default_value = ".")]
    workspace: PathBuf,

    #[arg(long)]
    state_root: Option<PathBuf>,

    #[arg(long)]
    output: Option<PathBuf>,

    #[arg(long, env = "DEEPSEEK_API_KEY", hide_env_values = true)]
    api_key: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut options = DeepseekCacheProbeOptions {
        model: cli.model,
        pairs: NonZeroUsize::new(cli.pairs).context("--pairs must be greater than zero")?,
        turns_per_arm: NonZeroUsize::new(cli.turns_per_arm)
            .context("--turns-per-arm must be greater than zero")?,
        max_output_tokens: NonZeroU32::new(cli.max_output_tokens)
            .context("--max-output-tokens must be greater than zero")?,
        ..DeepseekCacheProbeOptions::default()
    };
    if let Some(state_root) = cli.state_root {
        options.state_root = state_root;
    }
    options.validate()?;

    eprintln!(
        "Planned DeepSeek model turns: {} (maximum output tokens per attempt: {}; transport retries may add attempts)",
        options.planned_request_count(),
        options.max_output_tokens
    );
    if !cli.live || !cli.acknowledge_cost {
        eprintln!(
            "Dry run only. Pass both --live and --acknowledge-cost to call the official DeepSeek API."
        );
        return Ok(());
    }

    let api_key = cli
        .api_key
        .context("set DEEPSEEK_API_KEY or pass --api-key for a live probe")?;
    let mut config = RaraConfig::default();
    config.set_provider("deepseek");
    config.set_api_key(api_key);

    let report = run_deepseek_cache_probe(&config, &cli.workspace, options).await?;
    let output = cli
        .output
        .unwrap_or_else(|| PathBuf::from(format!("deepseek-cache-probe-{}.jsonl", Uuid::new_v4())));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&output)
        .with_context(|| format!("create probe report {}", output.display()))?;
    let mut writer = BufWriter::new(file);
    report.write_jsonl(&mut writer)?;
    writer.flush()?;

    eprintln!("Wrote content-free probe report to {}", output.display());
    if let Some(delta) = report.summary.cache_hit_rate_delta_basis_points {
        eprintln!("Stable-minus-busted cache hit-rate delta: {delta} basis points");
    }
    if let Some(reason) = &report.summary.inconclusive_reason {
        eprintln!("Probe is inconclusive: {reason}");
    }
    Ok(())
}
