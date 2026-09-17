use super::*;

impl TuiApp {
    pub fn new(cm: ConfigManager) -> anyhow::Result<Self> {
        let mut cfg = cm.load()?;
        cfg.apply_provider_environment_defaults();
        crate::tui::theme::install_config(&cfg.tui.theme);
        let overlay = None;
        let startup_notice = startup_warning_for_config(&cfg);
        let provider_picker_idx = selected_provider_family_idx_for_config(&cfg);
        let model_picker_idx = selected_preset_idx_for_config(&cfg, provider_picker_idx);
        let sandbox_network = cfg.sandbox_workspace_write.network_access;
        let mut app = Self {
            bottom_pane: BottomPaneModel {
                input: String::new(),
                input_cursor_offset: None,
                notice: startup_notice,
                ..Default::default()
            },
            input_history: Vec::new(),
            input_history_cursor: None,
            input_history_draft: None,
            committed_turns: Vec::new(),
            active_turn: TranscriptTurn::default(),
            overlay,
            overlay_stack: Vec::new(),
            sidebar_visible: true,
            thinking_collapsed: false,
            config: cfg,
            config_manager: cm,
            setup_status: None,
            runtime_phase: RuntimePhase::Idle,
            runtime_phase_detail: None,
            snapshot: RuntimeSnapshot::default(),
            agent_execution_mode: AgentExecutionMode::Execute,
            bash_approval_mode: BashApprovalMode::Suggestion,
            provider_picker_idx,
            model_picker_idx,
            openai_endpoint_kind_picker_idx: 0,
            openai_profile_picker_idx: 0,
            reasoning_effort_picker_idx: 0,
            auth_mode_idx: 0,
            nowledge_mem_picker_idx: 0,
            approval_picker_idx: 0,
            permission_picker_idx: 0,
            command_palette_idx: 0,
            model_search_query: String::new(),
            model_search_cursor_offset: None,
            model_search_idx: 0,
            base_url_input: String::new(),
            base_url_cursor_offset: None,
            api_key_input: String::new(),
            api_key_cursor_offset: None,
            model_name_input: String::new(),
            model_name_cursor_offset: None,
            openai_profile_label_input: String::new(),
            openai_profile_label_cursor_offset: None,
            openai_profile_label_kind: None,
            openai_setup_steps: Vec::new(),
            openai_setup_keep_empty_api_key: false,
            codex_model_options: Vec::new(),
            deepseek_model_options: fallback_models(ModelCatalogProvider::DeepSeek),
            kimi_model_options: fallback_models(ModelCatalogProvider::Kimi),
            deepseek_model_context_windows: rara_provider_catalog::fallback_catalog(
                ModelCatalogProvider::DeepSeek,
            )
            .into_iter()
            .filter_map(|entry| entry.context_window.map(|window| (entry.id, window)))
            .collect(),
            kimi_model_context_windows: rara_provider_catalog::fallback_catalog(
                ModelCatalogProvider::Kimi,
            )
            .into_iter()
            .filter_map(|entry| entry.context_window.map(|window| (entry.id, window)))
            .collect(),
            recent_commands: Vec::new(),
            recent_threads: Vec::new(),
            resume_picker_idx: 0,
            resume_sort_by_created: false,
            resume_search_query: String::new(),
            committed_render_generation: 0,
            committed_render_cache: RefCell::new(CommittedTranscriptRenderCache::default()),
            transcript_scroll: 0,
            transcript_selection: crate::tui::selection::TranscriptSelection::default(),
            context_scroll: 0,
            terminal_width: 80,
            agent_markdown_stream: None,
            agent_thinking_stream: None,
            active_live: ActiveLiveSections::default(),
            running_tool_boundary_count: 0,
            terminal_focused: true,
            state_db: None,
            state_db_status: None,
            shared_task_root: None,
            shared_task_fingerprint: None,
            shared_task_last_poll: None,
            mcp_manager: None,
            lsp_manager: None,
            #[cfg(test)]
            prompt_source_registry: None,
            #[cfg(test)]
            skill_source_registry: None,
            #[cfg(test)]
            hook_registry: None,
            hook_runtime: None,
            explicit_plugin_dirs: Vec::new(),
            memory_handler: None,
            provider_connection_status: std::collections::HashMap::new(),
            repo_context_task: None,
            repo_slug: None,
            current_pr_url: None,
            codex_auth_mode: None,
            skill_picker_idx: 0,
            skill_picker_entries: Vec::new(),
            sandbox_network_access: Arc::new(AtomicBool::new(sandbox_network)),
            permission_mode: PermissionMode::Custom,
            pending_permission_mode: None,
            goal: None,
            goal_handle: Arc::new(std::sync::RwLock::new(None)),
            event_bus: None,
            mcp_tool_cache: None,
        };

        app.permission_mode = app.effective_permission_mode();
        app.set_deepseek_model_catalog_with_source(
            rara_provider_catalog::fallback_catalog(ModelCatalogProvider::DeepSeek),
            true,
        );
        app.set_kimi_model_catalog_with_source(
            rara_provider_catalog::fallback_catalog(ModelCatalogProvider::Kimi),
            true,
        );
        app.refresh_provider_connection_status();
        app.refresh_recent_threads();

        Ok(app)
    }

    pub fn start_repo_context_detection(&mut self) {
        if self.repo_context_task.is_some() {
            return;
        }

        self.repo_context_task = Some(tokio::task::spawn_blocking(detect_repo_context));
    }

    pub async fn finish_repo_context_task_if_ready(&mut self) {
        let should_finish = self
            .repo_context_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished);
        if !should_finish {
            return;
        }

        let handle = self
            .repo_context_task
            .take()
            .expect("repo context task should exist");
        if let Ok((repo_slug, current_pr_url)) = handle.await {
            self.repo_slug = repo_slug;
            self.current_pr_url = current_pr_url;
        }
    }

    pub fn is_busy(&self) -> bool {
        self.bottom_pane.running_task.is_some()
    }

    pub fn running_elapsed(&self) -> Option<std::time::Duration> {
        self.bottom_pane
            .running_task
            .as_ref()
            .map(|task| task.started_at.elapsed())
    }

    pub fn current_model_label(&self) -> &str {
        self.config.model.as_deref().unwrap_or("-")
    }

    pub fn model_routing_view(&self) -> ModelRoutingView {
        let surface = self.config.effective_provider_surface();
        let main_model = surface
            .model
            .value
            .unwrap_or_else(|| self.current_model_label())
            .to_string();
        if let Some(auxiliary_model) = surface
            .auxiliary_model
            .value
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            return ModelRoutingView {
                main_model,
                main_source: surface.model.source.label().to_string(),
                auxiliary_model: auxiliary_model.to_string(),
                auxiliary_source: surface.auxiliary_model.source.label().to_string(),
                auxiliary_route: "configured".to_string(),
                auxiliary_uses_main_model: false,
            };
        }

        if let Some(auxiliary_model) = self.inferred_auxiliary_model(&main_model) {
            return ModelRoutingView {
                main_model,
                main_source: surface.model.source.label().to_string(),
                auxiliary_model,
                auxiliary_source: "inferred".to_string(),
                auxiliary_route: "provider_lite".to_string(),
                auxiliary_uses_main_model: false,
            };
        }

        ModelRoutingView {
            auxiliary_model: main_model.clone(),
            main_model,
            main_source: surface.model.source.label().to_string(),
            auxiliary_source: "main_model".to_string(),
            auxiliary_route: "fallback".to_string(),
            auxiliary_uses_main_model: true,
        }
    }

    pub(super) fn inferred_auxiliary_model(&self, main_model: &str) -> Option<String> {
        let endpoint_kind = self.config.active_openai_profile_kind()?;
        crate::llm::infer_openai_compatible_auxiliary_model(main_model, endpoint_kind)
            .map(|model| model.into_owned())
    }

    pub fn terminal_diagnostics_view(&self) -> TerminalDiagnosticsView {
        let info = rara_terminal_detection::terminal_info();
        TerminalDiagnosticsView {
            name: format!("{:?}", info.name),
            user_agent: info.user_agent_token(),
            term_program: info.term_program.clone(),
            term: info.term.clone(),
            multiplexer: terminal_multiplexer_label(info.multiplexer.as_ref()),
            remote: terminal_remote_label(info.remote.as_ref()).to_string(),
            history_mode: if info.is_zellij() {
                "zellij-fallback-insert".to_string()
            } else {
                "scroll-region".to_string()
            },
            focused: self.terminal_focused,
            width_columns: self.terminal_width,
        }
    }

    pub fn repo_context_hint(&self) -> Option<String> {
        let branch = self.snapshot.branch.trim();
        let mut parts = Vec::new();

        if let Some(repo_slug) = self
            .repo_slug
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            parts.push(format!("repo: {repo_slug}"));
        }

        if !branch.is_empty() {
            parts.push(format!("branch: {branch}"));
        }

        if let Some(pr_url) = self
            .current_pr_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            parts.push(format!("PR: {pr_url}"));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("  "))
        }
    }
}
