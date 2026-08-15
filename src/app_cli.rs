use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use rara_persistence::redaction::redact_secrets;
use secrecy::{ExposeSecret, SecretString};

use crate::acp::RaraAcpAgent;
use crate::config::{
    ConfigManager, DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_CHATGPT_BASE_URL, DEFAULT_KIMI_BASE_URL,
    DEFAULT_KIMI_MODEL, DEFAULT_REASONING_SUMMARY, NowledgeMemMode, OpenAiEndpointKind,
    OpenAiEndpointProfile, RaraConfig, ensure_rara_home_dir,
};
use crate::oauth::{OAuthManager, SavedCodexAuthMode};
use crate::plugin_cli::{PluginCommands, run_plugin_command};
use crate::print_consumer::PrintConsumer;
use crate::runtime_context;
use crate::thread_cli;
use crate::tui::StartupResumeTarget;
use crate::wire_consumer::WireConsumer;

#[derive(Parser)]
#[command(name = "rara")]
#[command(version, about = "RARA: RARA Automates Rust Agents", long_about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short, long, global = true)]
    provider: Option<String>,

    #[arg(short, long, env = "RARA_API_KEY", global = true)]
    api_key: Option<String>,

    #[arg(short, long, global = true)]
    base_url: Option<String>,

    #[arg(short, long, global = true)]
    model: Option<String>,

    #[arg(long, global = true)]
    revision: Option<String>,

    /// Additional Claude plugin directory to scan during TUI startup.
    #[arg(long = "plugin-dir", value_name = "DIR", global = true)]
    plugin_dirs: Vec<PathBuf>,
}

/// Register a model provider.
#[derive(Debug, clap::Args)]
struct ConnectArgs {
    /// Provider kind to register (e.g. deepseek, kimi, openrouter, custom)
    #[arg(long = "kind", short = 'k')]
    kind: Option<String>,

    /// Custom profile ID (defaults to the kind name, e.g. "deepseek")
    #[arg(long = "profile-id")]
    profile_id: Option<String>,

    /// Label for this profile
    #[arg(long)]
    label: Option<String>,

    /// API key for the provider
    #[arg(long)]
    api_key: Option<String>,

    /// Base URL override
    #[arg(long)]
    base_url: Option<String>,

    /// Default model for this profile
    #[arg(long)]
    model: Option<String>,

    /// Model revision / version
    #[arg(long)]
    revision: Option<String>,
}

#[derive(Debug, clap::Args)]
struct ModelsListArgs {
    /// Filter by provider kind (e.g. deepseek, kimi, openrouter, custom)
    #[arg(long = "kind", short = 'k')]
    kind: Option<String>,
}

#[derive(Debug, clap::Args)]
struct ModelsShowArgs {
    /// Profile ID to show
    profile_id: String,
}

#[derive(Debug, clap::Args)]
struct MemArgs {
    /// Nowledge Mem Cloud API key to save.
    #[arg(long)]
    api_key: String,
}

#[derive(Debug, clap::Args)]
struct ExecArgs {
    /// Print headless execution events to stdout as JSONL.
    #[arg(long, default_value_t = false)]
    json: bool,

    /// Working directory for the headless run.
    #[arg(long = "cwd", short = 'C', value_name = "DIR")]
    cwd: Option<PathBuf>,

    /// Write the final assistant message to this file.
    #[arg(long = "output-last-message", short = 'o', value_name = "FILE")]
    output_last_message: Option<PathBuf>,

    /// External benchmark or harness run id to include in JSONL metadata.
    #[arg(long = "run-id", value_name = "ID")]
    run_id: Option<String>,

    /// External benchmark task id to include in JSONL metadata.
    #[arg(long = "task-id", value_name = "ID")]
    task_id: Option<String>,

    /// Run headless automation with full shell access inside the selected workspace/container.
    #[arg(long = "full-access", default_value_t = false)]
    full_access: bool,

    /// Initial instructions. If omitted, or if `-` is used, read from stdin.
    #[arg(value_name = "PROMPT")]
    prompt: Option<String>,
}

#[derive(Debug, clap::Subcommand)]
enum ModelsCommands {
    /// List configured models
    List(ModelsListArgs),
    /// Show a single model profile
    Show(ModelsShowArgs),
}

#[derive(Subcommand, Debug)]
enum Commands {
    Acp,
    /// Register a model provider
    Connect(ConnectArgs),
    /// List, show, or select models
    #[command(subcommand)]
    Models(ModelsCommands),
    /// Install, list, or remove workspace plugins
    #[command(subcommand)]
    Plugin(PluginCommands),
    /// Configure the builtin Nowledge Mem Cloud integration.
    Mem(MemArgs),
    Ask {
        prompt: String,
    },
    /// Run RARA non-interactively for automation and evaluation harnesses.
    Exec(ExecArgs),
    Fork {
        #[arg(value_name = "THREAD_ID")]
        thread_id: String,
    },
    Distill {
        #[arg(value_name = "THREAD_ID")]
        thread_id: String,
    },
    Thread {
        #[arg(value_name = "THREAD_ID")]
        thread_id: String,
    },
    Threads {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Resume {
        #[arg(value_name = "THREAD_ID")]
        thread_id: Option<String>,
        #[arg(long, conflicts_with = "thread_id")]
        last: bool,
    },
    Login {
        #[arg(long)]
        device_auth: bool,
        #[arg(long)]
        with_api_key: bool,
    },
    Logout,
    Print {
        /// The prompt to send to the agent.
        prompt: String,
    },
    Wire {
        /// The prompt to send to the agent.
        prompt: String,
    },
    Tui,
}

pub(crate) async fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    let cli_plugin_dirs = cli.plugin_dirs.clone();
    let config_manager = ConfigManager::new()?;
    let mut config = config_manager.load()?;
    let command = apply_cli_overrides(&mut config, cli);
    if matches!(command, Some(Commands::Mem(_))) {
        config_manager.save(&config)?;
        println!("Nowledge Mem Cloud API key saved.");
        return Ok(());
    }
    let plugin_dirs = effective_plugin_dirs(&config, &cli_plugin_dirs)?;
    config.apply_provider_environment_defaults();
    let mem_config = &config.builtin_plugins.nowledge_mem;
    if let Some(api_key) = mem_config.api_key() {
        set_process_mem_api_key(&mem_config.api_key_env_var, api_key.to_string());
    }

    let oauth_manager = OAuthManager::new()?;

    match command.unwrap_or(Commands::Tui) {
        Commands::Acp => run_acp_command(&config, plugin_dirs).await?,
        Commands::Connect(args) => run_connect_command(&config, args)?,
        Commands::Models(cmd) => run_models_command(&config, cmd)?,
        Commands::Plugin(cmd) => run_plugin_command(cmd)?,
        Commands::Mem(_) => unreachable!("mem configuration returns before runtime startup"),
        Commands::Ask { prompt } => run_ask_command(&config, prompt, plugin_dirs).await?,
        Commands::Exec(args) => run_exec_command(&config, args, plugin_dirs).await?,
        Commands::Fork { thread_id } => thread_cli::run_fork_command(&thread_id)?,
        Commands::Distill { thread_id } => run_distill_command(&config, &thread_id).await?,
        Commands::Thread { thread_id } => thread_cli::run_thread_command(&thread_id)?,
        Commands::Threads { limit } => thread_cli::run_threads_command(limit)?,
        Commands::Resume { thread_id, last } => {
            run_tui_command(
                &config,
                oauth_manager,
                startup_resume_target_for_command(&Commands::Resume { thread_id, last })
                    .expect("resume command should always map to a startup target"),
                plugin_dirs,
            )
            .await?
        }
        Commands::Login {
            device_auth,
            with_api_key,
        } => {
            run_login_command(
                &mut config,
                &config_manager,
                &oauth_manager,
                device_auth,
                with_api_key,
            )
            .await?
        }
        Commands::Logout => run_logout_command(&mut config, &config_manager, &oauth_manager)?,
        Commands::Print { prompt } => run_print_command(&config, prompt, plugin_dirs).await?,
        Commands::Wire { prompt } => run_wire_command(&config, prompt, plugin_dirs).await?,
        Commands::Tui => {
            run_tui_command(
                &config,
                oauth_manager,
                startup_resume_target_for_command(&Commands::Tui)
                    .expect("tui command should always map to a startup target"),
                plugin_dirs,
            )
            .await?
        }
    }
    Ok(())
}

fn apply_cli_overrides(config: &mut RaraConfig, cli: Cli) -> Option<Commands> {
    if let Some(provider) = cli.provider {
        config.set_provider(provider);
    }
    if let Some(api_key) = cli.api_key {
        config.set_api_key(api_key);
    }
    if let Some(base_url) = cli.base_url {
        config.set_base_url(Some(base_url));
    }
    if let Some(model) = cli.model {
        config.set_model(Some(model));
    }
    if let Some(revision) = cli.revision {
        config.set_revision(Some(revision));
    }
    if let Some(Commands::Mem(args)) = cli.command.as_ref() {
        config.builtin_plugins.nowledge_mem.enabled = true;
        config.builtin_plugins.nowledge_mem.mode = NowledgeMemMode::Cloud;
        config
            .builtin_plugins
            .nowledge_mem
            .set_api_key(args.api_key.clone());
    }
    cli.command
}

fn set_process_mem_api_key(env_var: &str, api_key: String) {
    // SAFETY: CLI overrides are applied before RARA starts async runtimes or
    // worker threads, so no concurrent environment access is possible here.
    unsafe { std::env::set_var(env_var, api_key) };
}

fn normalize_plugin_dirs(plugin_dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    plugin_dirs
        .iter()
        .map(|path| normalize_plugin_dir(path))
        .collect()
}

fn effective_plugin_dirs(config: &RaraConfig, cli_plugin_dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut plugin_dirs = config.plugin_dirs.clone();
    plugin_dirs.extend_from_slice(cli_plugin_dirs);
    let normalized = normalize_plugin_dirs(&plugin_dirs)?;
    let mut seen = std::collections::HashSet::new();
    Ok(normalized
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect())
}

fn normalize_plugin_dir(path: &Path) -> Result<PathBuf> {
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(_) if path.is_absolute() => path.to_path_buf(),
        Err(_) => std::env::current_dir()?.join(path),
    };
    Ok(normalize_path_components(path))
}

fn normalize_path_components(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

async fn run_acp_command(config: &RaraConfig, plugin_dirs: Vec<PathBuf>) -> Result<()> {
    let acp_agent = RaraAcpAgent::new(config.clone(), plugin_dirs);
    acp_agent
        .run_acp_stdio()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

async fn run_ask_command(
    config: &RaraConfig,
    prompt: String,
    plugin_dirs: Vec<PathBuf>,
) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context_for_workspace_with_options(
        config,
        None,
        None,
        runtime_context::RuntimeBootstrapOptions::with_plugin_dirs(plugin_dirs),
    )
    .await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let mut agent = bootstrap.into_agent().await;
    agent.query(prompt).await
}

async fn run_print_command(
    config: &RaraConfig,
    prompt: String,
    plugin_dirs: Vec<PathBuf>,
) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context_for_workspace_with_options(
        config,
        None,
        None,
        runtime_context::RuntimeBootstrapOptions::with_plugin_dirs(plugin_dirs),
    )
    .await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let event_bus = bootstrap.event_bus.clone();
    let agent = bootstrap.into_agent().await;
    let consumer = PrintConsumer::new(agent, event_bus, prompt);
    consumer.run().await
}

async fn run_exec_command(
    config: &RaraConfig,
    args: ExecArgs,
    plugin_dirs: Vec<PathBuf>,
) -> Result<()> {
    let startup_complete = install_exec_panic_hook(&args);
    if let Some(cwd) = args.cwd.as_deref() {
        std::env::set_current_dir(cwd)
            .map_err(|err| anyhow::anyhow!("failed to switch cwd to {}: {err}", cwd.display()))?;
    }
    let bootstrap = runtime_context::initialize_rara_context_for_workspace_with_options(
        config,
        None,
        None,
        runtime_context::RuntimeBootstrapOptions::with_plugin_dirs(plugin_dirs),
    )
    .await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let mut agent = bootstrap.into_agent().await;
    if args.full_access {
        agent.set_full_access_mode(true);
    }
    let consumer = crate::exec_consumer::ExecConsumer::new(
        agent,
        crate::exec_consumer::ExecRunOptions {
            prompt: args.prompt,
            json: args.json,
            output_last_message: args.output_last_message,
            run_id: args.run_id,
            task_id: args.task_id,
        },
    );
    if let Some(startup_complete) = startup_complete {
        startup_complete.store(true, Ordering::Release);
    }
    consumer.run().await
}

fn install_exec_panic_hook(args: &ExecArgs) -> Option<Arc<AtomicBool>> {
    if !args.json {
        return None;
    }
    let startup_complete = Arc::new(AtomicBool::new(false));
    let startup_complete_for_hook = startup_complete.clone();
    let run_id = args.run_id.clone();
    let task_id = args.task_id.clone();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !startup_complete_for_hook.load(Ordering::Acquire) {
            crate::exec_consumer::emit_exec_startup_failure_jsonl(
                run_id.clone(),
                task_id.clone(),
                format!("rara exec panicked during startup: {info}"),
            );
        }
        default_hook(info);
    }));
    Some(startup_complete)
}

async fn run_wire_command(
    config: &RaraConfig,
    prompt: String,
    plugin_dirs: Vec<PathBuf>,
) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context_for_workspace_with_options(
        config,
        None,
        None,
        runtime_context::RuntimeBootstrapOptions::with_plugin_dirs(plugin_dirs),
    )
    .await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let event_bus = bootstrap.event_bus.clone();
    let agent = bootstrap.into_agent().await;
    let consumer = WireConsumer::new(agent, event_bus, prompt);
    consumer.run().await
}

async fn run_distill_command(config: &RaraConfig, thread_id: &str) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context(config, None).await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let state_db = rara_state::state_db::StateDb::new()?;
    let memory_store = crate::memory_store::MemoryStore::new_with_handle(
        bootstrap.backend.clone(),
        bootstrap.memory_handle.clone(),
    );
    let thread_store = crate::thread_store::ThreadStore::new(&bootstrap.session_manager, &state_db);
    let memories = thread_store
        .distill_thread_memories(&memory_store, thread_id)
        .await?;
    print!(
        "{}",
        thread_cli::format_distilled_memories(thread_id, &memories)
    );
    Ok(())
}

async fn run_tui_command(
    config: &RaraConfig,
    oauth_manager: OAuthManager,
    startup_resume: StartupResumeTarget,
    plugin_dirs: Vec<PathBuf>,
) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context_for_workspace_with_options(
        config,
        None,
        None,
        runtime_context::RuntimeBootstrapOptions::with_plugin_dirs(plugin_dirs.clone()),
    )
    .await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let runtime_client = crate::runtime_client::RuntimeClient::from_bootstrap(bootstrap).await;
    let resumed_thread_id =
        crate::tui::run_tui(runtime_client, oauth_manager, startup_resume).await?;
    if let Some(thread_id) = resumed_thread_id {
        print!("{}", rendered_resume_hint(&thread_id));
    }
    Ok(())
}

fn startup_resume_target_for_command(command: &Commands) -> Option<StartupResumeTarget> {
    match command {
        Commands::Resume {
            thread_id: Some(thread_id),
            ..
        } => Some(StartupResumeTarget::ThreadId(thread_id.clone())),
        Commands::Resume {
            thread_id: None,
            last: true,
        } => Some(StartupResumeTarget::Latest),
        Commands::Resume {
            thread_id: None,
            last: false,
        } => Some(StartupResumeTarget::Picker),
        Commands::Tui => Some(StartupResumeTarget::Fresh),
        Commands::Acp
        | Commands::Connect(..)
        | Commands::Models(..)
        | Commands::Plugin(..)
        | Commands::Mem(..)
        | Commands::Ask { .. }
        | Commands::Exec(..)
        | Commands::Distill { .. }
        | Commands::Fork { .. }
        | Commands::Thread { .. }
        | Commands::Threads { .. }
        | Commands::Login { .. }
        | Commands::Logout
        | Commands::Print { .. }
        | Commands::Wire { .. } => None,
    }
}

async fn run_login_command(
    config: &mut RaraConfig,
    config_manager: &ConfigManager,
    oauth_manager: &OAuthManager,
    device_auth: bool,
    with_api_key: bool,
) -> Result<()> {
    if device_auth && with_api_key {
        bail!("choose either --device-auth or --with-api-key, not both");
    }
    if with_api_key {
        let oauth_reader = oauth_manager.clone();
        let api_key =
            tokio::task::spawn_blocking(move || oauth_reader.read_api_key_from_stdin()).await??;
        let credential = oauth_manager.save_api_key(api_key.expose_secret())?;
        save_codex_credential(
            config,
            config_manager,
            oauth_manager,
            credential.expose_secret(),
        )?;
        println!("Successfully saved Codex API key.");
        return Ok(());
    }
    if device_auth {
        let token = oauth_manager.request_device_code().await?;
        eprintln!(
            "Open this URL and enter the one-time code:\n{}\n\nCode: {}",
            token.verification_url, token.user_code
        );
        let credential = oauth_manager.complete_device_code_login(&token).await?;
        save_codex_credential(
            config,
            config_manager,
            oauth_manager,
            credential.expose_secret(),
        )?;
        println!("Successfully logged in with device code.");
        return Ok(());
    }

    if std::env::var_os("SSH_CONNECTION").is_some() {
        bail!(
            "browser login is not reliable in SSH/headless sessions; use --device-auth or --with-api-key"
        );
    }
    let session = oauth_manager.start_browser_login(true)?;
    eprintln!(
        "Starting local login flow.\nIf your browser did not open, navigate to this URL:\n\n{}",
        session.auth_url()
    );
    let credential = session.complete(oauth_manager).await?;
    save_codex_credential(
        config,
        config_manager,
        oauth_manager,
        credential.expose_secret(),
    )?;
    println!("Successfully logged in.");
    Ok(())
}

fn run_logout_command(
    config: &mut RaraConfig,
    config_manager: &ConfigManager,
    oauth_manager: &OAuthManager,
) -> Result<()> {
    let removed = oauth_manager.clear_saved_auth()?;
    config.clear_provider_api_key("codex");
    config_manager.save(config)?;
    if removed {
        println!("Removed the saved Codex credential.");
    } else {
        println!("No saved Codex credential was present.");
    }
    Ok(())
}

fn resume_hint(thread_id: &str) -> String {
    format!("Resume this thread with: rara resume {thread_id}")
}

fn rendered_resume_hint(thread_id: &str) -> String {
    format!("\n{}\n", resume_hint(thread_id))
}

fn emit_bootstrap_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("{}", redact_secrets(format!("Warning: {warning}")));
    }
}

fn save_codex_credential(
    config: &mut RaraConfig,
    config_manager: &ConfigManager,
    oauth_manager: &OAuthManager,
    credential: &str,
) -> Result<()> {
    config.set_provider("codex");
    config.set_api_key(credential.to_string());
    let base_url = match oauth_manager.saved_auth_mode()? {
        Some(SavedCodexAuthMode::Chatgpt) => DEFAULT_CODEX_CHATGPT_BASE_URL,
        _ => DEFAULT_CODEX_BASE_URL,
    };
    config.apply_codex_defaults_for_base_url(base_url);
    config_manager.save(config)
}

fn run_connect_command(config: &RaraConfig, args: ConnectArgs) -> Result<()> {
    let kind = parse_endpoint_kind(args.kind.as_deref().unwrap_or("custom"))?;
    let mut config = config.clone();
    let profile_id = args
        .profile_id
        .clone()
        .unwrap_or_else(|| kind.default_profile_id().to_string());
    let profile = config
        .openai_profiles
        .entry(profile_id.clone())
        .or_insert_with(|| OpenAiEndpointProfile {
            id: profile_id.clone(),
            label: kind.label().to_string(),
            kind,
            api_key: None,
            base_url: Some(kind.default_base_url().to_string()),
            model: Some(kind.default_model().to_string()),
            auxiliary_model: None,
            reasoning_effort: None,
            reasoning_summary: Some(DEFAULT_REASONING_SUMMARY.to_string()),
            revision: None,
        });
    if let Some(api_key) = args.api_key {
        profile.api_key = Some(SecretString::from(api_key));
    }
    if let Some(base_url) = args.base_url {
        profile.base_url = Some(base_url);
    }
    if let Some(model) = args.model {
        profile.model = Some(model);
    }
    if let Some(label) = args.label {
        profile.label = label;
    }
    if let Some(revision) = args.revision {
        profile.revision = Some(revision);
    }

    let config_manager = ConfigManager::new()?;
    config_manager.save(&config)?;
    println!(
        "Registered provider profile '{}' (kind={}).",
        profile_id,
        kind.label()
    );
    Ok(())
}

fn run_models_command(config: &RaraConfig, cmd: ModelsCommands) -> Result<()> {
    match cmd {
        ModelsCommands::List(args) => run_models_list(config, args),
        ModelsCommands::Show(args) => run_models_show(config, args),
    }
}

fn run_models_list(config: &RaraConfig, args: ModelsListArgs) -> Result<()> {
    let profiles: Vec<_> = if let Some(ref kind_filter) = args.kind {
        let kind = parse_endpoint_kind(kind_filter.as_str())?;
        config
            .openai_profiles
            .iter()
            .filter(|(_, p)| p.kind == kind)
            .collect()
    } else {
        config.openai_profiles.iter().collect()
    };

    if profiles.is_empty() {
        if args.kind.is_some() {
            println!("No profiles found for the given kind.");
        } else {
            println!("No model profiles configured. Use 'rara connect' to register a provider.");
        }
        return Ok(());
    }

    println!("Configured model profiles:\n");
    for (id, profile) in &profiles {
        println!(
            "  {:<25} kind={:<12} model={:?}   base={:?}",
            id,
            profile.kind.label(),
            profile.model.as_deref().unwrap_or("-"),
            profile.base_url.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}

fn run_models_show(config: &RaraConfig, args: ModelsShowArgs) -> Result<()> {
    let profile = config
        .openai_profiles
        .get(&args.profile_id)
        .ok_or_else(|| anyhow::anyhow!("profile '{}' not found", args.profile_id))?;

    println!("Profile: {}", profile.id);
    println!("  kind:      {}", profile.kind.label());
    println!("  model:     {}", profile.model.as_deref().unwrap_or("-"));
    println!(
        "  base_url:  {}",
        profile.base_url.as_deref().unwrap_or("-")
    );
    if !profile.label.is_empty() {
        println!("  label:     {}", profile.label);
    }
    Ok(())
}

fn parse_endpoint_kind(kind_str: &str) -> Result<OpenAiEndpointKind> {
    match kind_str.to_lowercase().as_str() {
        "deepseek" => Ok(OpenAiEndpointKind::Deepseek),
        "kimi" => Ok(OpenAiEndpointKind::Kimi),
        "kimi-coding" => Ok(OpenAiEndpointKind::KimiCoding),
        "openrouter" => Ok(OpenAiEndpointKind::Openrouter),
        "custom" | "openai-compatible" => Ok(OpenAiEndpointKind::Custom),
        other => Err(anyhow::anyhow!(
            "unknown provider kind '{}'. Supported kinds: deepseek, kimi, kimi-coding, openrouter, custom",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clap_parses_ask_command() {
        let cli = Cli::try_parse_from(["rara", "ask", "hello"]).expect("parse ask");
        match cli.command.expect("command") {
            Commands::Ask { prompt } => assert_eq!(prompt, "hello"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn mem_api_key_cli_override_persists_cloud_configuration() {
        let cli = Cli::try_parse_from(["rara", "mem", "--api-key", "nmem_test_key"])
            .expect("parse mem api key");
        let mut config = RaraConfig::default();
        config.builtin_plugins.nowledge_mem.api_key_env_var = "CUSTOM_MEM_KEY".to_string();

        assert!(matches!(
            apply_cli_overrides(&mut config, cli),
            Some(Commands::Mem(_))
        ));
        assert_eq!(
            config.builtin_plugins.nowledge_mem.api_key_env_var,
            "CUSTOM_MEM_KEY"
        );
        assert_eq!(
            config.builtin_plugins.nowledge_mem.api_key(),
            Some("nmem_test_key")
        );
        let serialized = serde_json::to_string(&config).expect("serialize config");
        assert!(serialized.contains("nmem_test_key"));
    }

    #[test]
    fn clap_parses_explicit_plugin_dirs_as_global_args() {
        let cli = Cli::try_parse_from([
            "rara",
            "--plugin-dir",
            "plugins-a",
            "--plugin-dir",
            "plugins-b",
            "tui",
        ])
        .expect("parse plugin dirs");

        assert_eq!(
            cli.plugin_dirs,
            vec![PathBuf::from("plugins-a"), PathBuf::from("plugins-b")]
        );
        assert!(matches!(cli.command, Some(Commands::Tui)));
    }

    #[test]
    fn clap_parses_explicit_plugin_dirs_after_tui_command() {
        let cli = Cli::try_parse_from(["rara", "tui", "--plugin-dir", "plugins-a"])
            .expect("parse plugin dir after tui command");

        assert_eq!(cli.plugin_dirs, vec![PathBuf::from("plugins-a")]);
        assert!(matches!(cli.command, Some(Commands::Tui)));
    }

    #[test]
    fn normalize_plugin_dirs_returns_absolute_paths() {
        let cwd = std::env::current_dir().expect("cwd");
        let normalized = normalize_plugin_dirs(&[
            PathBuf::from("."),
            PathBuf::from("missing-plugin-dir"),
            cwd.join("missing-absolute-plugin-dir"),
        ])
        .expect("normalize plugin dirs");

        assert_eq!(normalized[0], cwd.canonicalize().expect("canonical cwd"));
        assert_eq!(normalized[1], cwd.join("missing-plugin-dir"));
        assert_eq!(normalized[2], cwd.join("missing-absolute-plugin-dir"));
        assert!(normalized.iter().all(|path| path.is_absolute()));
    }

    #[test]
    fn effective_plugin_dirs_put_cli_dirs_after_config_dirs_and_deduplicates() {
        let cwd = std::env::current_dir().expect("cwd");
        let config = RaraConfig {
            plugin_dirs: vec![
                PathBuf::from("config-plugins"),
                PathBuf::from("./config-plugins"),
            ],
            ..Default::default()
        };

        let normalized = effective_plugin_dirs(
            &config,
            &[
                PathBuf::from("cli-plugins"),
                PathBuf::from("config-plugins"),
            ],
        )
        .expect("effective plugin dirs");

        assert_eq!(
            normalized,
            vec![cwd.join("config-plugins"), cwd.join("cli-plugins")]
        );
    }

    #[test]
    fn clap_parses_exec_command_for_headless_harnesses() {
        let cli = Cli::try_parse_from([
            "rara",
            "exec",
            "--json",
            "-C",
            "task-workspace",
            "--run-id",
            "run-1",
            "--task-id",
            "task-1",
            "--output-last-message",
            "final.txt",
            "--full-access",
            "-",
        ])
        .expect("parse exec");
        match cli.command.expect("command") {
            Commands::Exec(args) => {
                assert!(args.json);
                assert_eq!(args.cwd, Some(PathBuf::from("task-workspace")));
                assert_eq!(args.run_id.as_deref(), Some("run-1"));
                assert_eq!(args.task_id.as_deref(), Some("task-1"));
                assert_eq!(args.output_last_message, Some(PathBuf::from("final.txt")));
                assert!(args.full_access);
                assert_eq!(args.prompt.as_deref(), Some("-"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clap_parses_thread_resume_with_last() {
        let cli = Cli::try_parse_from(["rara", "resume", "--last"]).expect("parse resume --last");
        match cli.command.expect("command") {
            Commands::Resume { thread_id, last } => {
                assert_eq!(thread_id, None);
                assert!(last);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clap_parses_fork_command() {
        let cli = Cli::try_parse_from(["rara", "fork", "thread-123"]).expect("parse fork");
        match cli.command.expect("command") {
            Commands::Fork { thread_id } => {
                assert_eq!(thread_id, "thread-123");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clap_parses_distill_command() {
        let cli = Cli::try_parse_from(["rara", "distill", "thread-123"]).expect("parse distill");
        match cli.command.expect("command") {
            Commands::Distill { thread_id } => {
                assert_eq!(thread_id, "thread-123");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clap_supports_version_flag_for_release_smoke_tests() {
        let err = match Cli::try_parse_from(["rara", "--version"]) {
            Ok(_) => panic!("version should exit early"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
        assert!(err.to_string().starts_with("rara "));
    }

    #[test]
    fn startup_resume_targets_are_explicit() {
        assert!(matches!(
            startup_resume_target_for_command(&Commands::Tui),
            Some(StartupResumeTarget::Fresh)
        ));
        assert!(matches!(
            startup_resume_target_for_command(&Commands::Resume {
                thread_id: None,
                last: false
            }),
            Some(StartupResumeTarget::Picker)
        ));
        assert!(matches!(
            startup_resume_target_for_command(&Commands::Resume {
                thread_id: None,
                last: true
            }),
            Some(StartupResumeTarget::Latest)
        ));
        assert!(matches!(
            startup_resume_target_for_command(&Commands::Resume {
                thread_id: Some("thread-123".to_string()),
                last: false
            }),
            Some(StartupResumeTarget::ThreadId(thread_id)) if thread_id == "thread-123"
        ));
        assert!(
            startup_resume_target_for_command(&Commands::Exec(ExecArgs {
                json: true,
                cwd: None,
                output_last_message: None,
                run_id: None,
                task_id: None,
                full_access: false,
                prompt: Some("hello".to_string()),
            }))
            .is_none()
        );
    }

    // --- connect / models CLI parsing ---

    #[test]
    fn clap_parses_connect_all_args() {
        let cli = Cli::try_parse_from([
            "rara",
            "connect",
            "--kind",
            "deepseek",
            "--profile-id",
            "deepseek-v3",
            "--api-key",
            "sk-abc123",
            "--base-url",
            "https://api.deepseek.com/v1",
            "--model",
            "deepseek-v3",
            "--label",
            "my-deepseek",
            "--revision",
            "v3-0324",
        ])
        .expect("parse connect");
        match cli.command.expect("command") {
            Commands::Connect(args) => {
                assert_eq!(args.kind, Some("deepseek".to_string()));
                assert_eq!(args.profile_id, Some("deepseek-v3".to_string()));
                assert_eq!(args.api_key, Some("sk-abc123".to_string()));
                assert_eq!(
                    args.base_url,
                    Some("https://api.deepseek.com/v1".to_string())
                );
                assert_eq!(args.model, Some("deepseek-v3".to_string()));
                assert_eq!(args.label, Some("my-deepseek".to_string()));
                assert_eq!(args.revision, Some("v3-0324".to_string()));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clap_parses_connect_minimal() {
        let cli = Cli::try_parse_from(["rara", "connect"]).expect("parse connect");
        match cli.command.expect("command") {
            Commands::Connect(args) => {
                assert_eq!(args.kind, None);
                assert_eq!(args.profile_id, None);
                assert_eq!(args.api_key, None);
                assert_eq!(args.base_url, None);
                assert_eq!(args.model, None);
                assert_eq!(args.label, None);
                assert_eq!(args.revision, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clap_parses_models_list() {
        let cli = Cli::try_parse_from(["rara", "models", "list"]).expect("parse models list");
        match cli.command.expect("command") {
            Commands::Models(ModelsCommands::List(args)) => {
                assert_eq!(args.kind, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clap_parses_models_list_with_kind() {
        let cli = Cli::try_parse_from(["rara", "models", "list", "--kind", "kimi"])
            .expect("parse models list --kind");
        match cli.command.expect("command") {
            Commands::Models(ModelsCommands::List(args)) => {
                assert_eq!(args.kind, Some("kimi".to_string()));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clap_parses_models_show() {
        let cli =
            Cli::try_parse_from(["rara", "models", "show", "deepseek"]).expect("parse models show");
        match cli.command.expect("command") {
            Commands::Models(ModelsCommands::Show(args)) => {
                assert_eq!(args.profile_id, "deepseek");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clap_parses_plugin_install() {
        let cli = Cli::try_parse_from(["rara", "plugin", "install", "../my-plugin", "--force"])
            .expect("parse plugin install");
        match cli.command.expect("command") {
            Commands::Plugin(PluginCommands::Install(args)) => {
                assert_eq!(args.source, "../my-plugin");
                assert!(args.force);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clap_parses_plugin_remove() {
        let cli = Cli::try_parse_from(["rara", "plugin", "remove", "test-plugin"])
            .expect("parse plugin remove");
        match cli.command.expect("command") {
            Commands::Plugin(PluginCommands::Remove(args)) => {
                assert_eq!(args.name, "test-plugin");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    // --- parse_endpoint_kind ---

    #[test]
    fn parse_endpoint_kind_valid_variants() {
        assert_eq!(
            parse_endpoint_kind("deepseek").expect("deepseek"),
            OpenAiEndpointKind::Deepseek
        );
        assert_eq!(
            parse_endpoint_kind("DEEPSEEK").expect("upper"),
            OpenAiEndpointKind::Deepseek
        );
        assert_eq!(
            parse_endpoint_kind("kimi").expect("kimi"),
            OpenAiEndpointKind::Kimi
        );
        assert_eq!(
            parse_endpoint_kind("kimi-coding").expect("kimi-coding"),
            OpenAiEndpointKind::KimiCoding
        );
        assert_eq!(
            parse_endpoint_kind("openrouter").expect("openrouter"),
            OpenAiEndpointKind::Openrouter
        );
        assert_eq!(
            parse_endpoint_kind("custom").expect("custom"),
            OpenAiEndpointKind::Custom
        );
        assert_eq!(
            parse_endpoint_kind("openai-compatible").expect("compat"),
            OpenAiEndpointKind::Custom
        );
    }

    #[test]
    fn parse_endpoint_kind_unknown() {
        let err = parse_endpoint_kind("nonexistent").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("nonexistent"),
            "message should mention the bad kind: {msg}"
        );
    }

    // --- run_models_list / run_models_show ---

    fn config_with_profiles() -> RaraConfig {
        let mut config = RaraConfig::default();
        config.openai_profiles.insert(
            "deepseek".to_string(),
            OpenAiEndpointProfile {
                id: "deepseek".to_string(),
                label: "DeepSeek V3".to_string(),
                kind: OpenAiEndpointKind::Deepseek,
                api_key: None,
                base_url: Some("https://api.deepseek.com/v1".to_string()),
                model: Some("deepseek-chat".to_string()),
                auxiliary_model: None,
                reasoning_effort: None,
                reasoning_summary: Some(DEFAULT_REASONING_SUMMARY.to_string()),
                revision: None,
            },
        );
        config.openai_profiles.insert(
            "kimi".to_string(),
            OpenAiEndpointProfile {
                id: "kimi".to_string(),
                label: "Moonshot AI".to_string(),
                kind: OpenAiEndpointKind::Kimi,
                api_key: None,
                base_url: Some(DEFAULT_KIMI_BASE_URL.to_string()),
                model: Some(DEFAULT_KIMI_MODEL.to_string()),
                auxiliary_model: None,
                reasoning_effort: None,
                reasoning_summary: Some(DEFAULT_REASONING_SUMMARY.to_string()),
                revision: None,
            },
        );
        config
    }

    #[test]
    fn models_list_all() {
        let config = config_with_profiles();
        run_models_list(&config, ModelsListArgs { kind: None }).expect("list all");
    }

    #[test]
    fn models_list_filter_by_kind() {
        let config = config_with_profiles();
        run_models_list(
            &config,
            ModelsListArgs {
                kind: Some("deepseek".to_string()),
            },
        )
        .expect("list deepseek");
    }

    #[test]
    fn models_list_none_for_kind() {
        let config = config_with_profiles();
        run_models_list(
            &config,
            ModelsListArgs {
                kind: Some("openrouter".to_string()),
            },
        )
        .expect("list openrouter (none configured)");
    }

    #[test]
    fn models_list_empty() {
        let config = RaraConfig::default();
        run_models_list(&config, ModelsListArgs { kind: None }).expect("list empty");
    }

    #[test]
    fn models_show_existing() {
        let config = config_with_profiles();
        run_models_show(
            &config,
            ModelsShowArgs {
                profile_id: "deepseek".to_string(),
            },
        )
        .expect("show deepseek");
    }

    #[test]
    fn models_show_not_found() {
        let config = config_with_profiles();
        let err = run_models_show(
            &config,
            ModelsShowArgs {
                profile_id: "nonexistent".to_string(),
            },
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("nonexistent"),
            "message should mention missing id: {msg}"
        );
    }

    #[test]
    fn connect_and_models_startup_resume_targets_are_none() {
        // These commands skip TUI startup entirely.
        assert!(
            startup_resume_target_for_command(&Commands::Connect(ConnectArgs {
                kind: None,
                profile_id: None,
                api_key: None,
                base_url: None,
                model: None,
                label: None,
                revision: None,
            }))
            .is_none()
        );
        assert!(
            startup_resume_target_for_command(&Commands::Models(ModelsCommands::List(
                ModelsListArgs { kind: None }
            )))
            .is_none()
        );
        assert!(
            startup_resume_target_for_command(&Commands::Plugin(PluginCommands::List)).is_none()
        );
    }
}
