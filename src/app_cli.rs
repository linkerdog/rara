use std::sync::Arc;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use rara_persistence::redaction::redact_secrets;
use secrecy::ExposeSecret;

use crate::acp::RaraAcpAgent;
use crate::config::{
    ConfigManager, DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_CHATGPT_BASE_URL, RaraConfig,
};
use crate::oauth::{OAuthManager, SavedCodexAuthMode};
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
}

#[derive(Subcommand, Debug)]
enum Commands {
    Acp,
    Ask {
        prompt: String,
    },
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
    let config_manager = ConfigManager::new()?;
    let mut config = config_manager.load()?;
    let command = apply_cli_overrides(&mut config, cli);
    config.apply_provider_environment_defaults();

    let oauth_manager = OAuthManager::new()?;

    match command.unwrap_or(Commands::Tui) {
        Commands::Acp => run_acp_command(&config).await?,
        Commands::Ask { prompt } => run_ask_command(&config, prompt).await?,
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
        Commands::Print { prompt } => run_print_command(&config, prompt).await?,
        Commands::Wire { prompt } => run_wire_command(&config, prompt).await?,
        Commands::Tui => {
            run_tui_command(
                &config,
                oauth_manager,
                startup_resume_target_for_command(&Commands::Tui)
                    .expect("tui command should always map to a startup target"),
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
    cli.command
}

async fn run_acp_command(config: &RaraConfig) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context(config, None).await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let acp_agent = RaraAcpAgent::new(
        bootstrap.backend.clone(),
        Arc::new(bootstrap.tool_manager),
        bootstrap.event_bus.clone(),
    );
    acp_agent
        .run_acp_stdio()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

async fn run_ask_command(config: &RaraConfig, prompt: String) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context(config, None).await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let mut agent = bootstrap.into_agent();
    agent.query(prompt).await
}

async fn run_print_command(config: &RaraConfig, prompt: String) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context(config, None).await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let event_bus = bootstrap.event_bus.clone();
    let agent = bootstrap.into_agent();
    let consumer = PrintConsumer::new(agent, event_bus, prompt);
    consumer.run().await
}

async fn run_wire_command(config: &RaraConfig, prompt: String) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context(config, None).await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let event_bus = bootstrap.event_bus.clone();
    let agent = bootstrap.into_agent();
    let consumer = WireConsumer::new(agent, event_bus, prompt);
    consumer.run().await
}

async fn run_distill_command(config: &RaraConfig, thread_id: &str) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context(config, None).await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let state_db = rara_state::state_db::StateDb::new()?;
    let memory_store =
        crate::memory_store::MemoryStore::new(bootstrap.backend.clone(), bootstrap.vdb.clone());
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
) -> Result<()> {
    let bootstrap = runtime_context::initialize_rara_context(config, None).await?;
    emit_bootstrap_warnings(&bootstrap.warnings);
    let sandbox_network_access = bootstrap.sandbox_network_access.clone();
    let event_bus = bootstrap.event_bus.clone();
    let (agent, _warnings, _network, goal_handle, mcp_tool_cache) = bootstrap.into_parts();
    let resumed_thread_id = crate::tui::run_tui(
        agent,
        goal_handle,
        mcp_tool_cache,
        oauth_manager,
        startup_resume,
        sandbox_network_access,
        event_bus,
    )
    .await?;
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
        | Commands::Ask { .. }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_hint_includes_exact_command() {
        assert_eq!(
            resume_hint("thread-123"),
            "Resume this thread with: rara resume thread-123"
        );
    }

    #[test]
    fn rendered_resume_hint_starts_on_a_new_line() {
        assert_eq!(
            rendered_resume_hint("thread-123"),
            "\nResume this thread with: rara resume thread-123\n"
        );
    }

    #[test]
    fn clap_parses_resume_command_with_optional_thread_id() {
        let cli = Cli::try_parse_from(["rara", "resume", "thread-123"]).expect("parse resume");
        match cli.command.expect("command") {
            Commands::Resume { thread_id, last } => {
                assert_eq!(thread_id.as_deref(), Some("thread-123"));
                assert!(!last);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::try_parse_from(["rara", "resume"]).expect("parse resume without id");
        match cli.command.expect("command") {
            Commands::Resume { thread_id, last } => {
                assert_eq!(thread_id, None);
                assert!(!last);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::try_parse_from(["rara", "resume", "--last"]).expect("parse resume latest");
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
    }
}
