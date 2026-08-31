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
    fn cli_reasoning_overrides_apply_to_headless_config() {
        let cli = Cli::try_parse_from([
            "rara",
            "exec",
            "--reasoning-effort",
            "high",
            "--thinking",
            "true",
            "-",
        ])
        .expect("parse reasoning overrides");
        let mut config = RaraConfig::default();

        assert!(matches!(
            apply_cli_overrides(&mut config, cli),
            Some(Commands::Exec(_))
        ));
        assert_eq!(config.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(config.thinking, Some(true));
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
            "--runtime-profile",
            "headless-coding-v1",
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
                assert_eq!(
                    args.runtime_profile,
                    RuntimeSessionProfile::HeadlessCodingV1
                );
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
                runtime_profile: RuntimeSessionProfile::Default,
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
