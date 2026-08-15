use super::{
    ListPickerKind, OpenAiEndpointKind, Overlay, PROVIDER_FAMILIES, ProviderFamily, TuiApp,
    input_requests_command_palette, openai_profile_setup_kinds,
    selected_provider_family_idx_for_config,
};
use crate::tui::is_ssh_session;

impl TuiApp {
    pub fn open_overlay(&mut self, overlay: Overlay) {
        if matches!(overlay, Overlay::CommandPalette | Overlay::ModelSearch) {
            self.command_palette_idx = 0;
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::Provider)) {
            self.provider_picker_idx = selected_provider_family_idx_for_config(&self.config);
            self.refresh_provider_connection_status();
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::Resume)) {
            self.refresh_recent_threads_for_resume_picker();
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::UnifiedModel)) {
            self.refresh_provider_connection_status();
            self.model_picker_idx = self.selected_unified_preset_idx();
            self.sync_reasoning_effort_picker();
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::NowledgeMem)) {
            let config = &self.config.builtin_plugins.nowledge_mem;
            self.nowledge_mem_picker_idx = if !config.enabled {
                0
            } else if config.mode == crate::config::NowledgeMemMode::Cloud {
                2
            } else {
                1
            };
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::Model)) {
            let selected_family = self.selected_provider_family();
            if matches!(selected_family, ProviderFamily::OpenAiCompatible) {
                if !matches!(
                    PROVIDER_FAMILIES
                        .get(selected_provider_family_idx_for_config(&self.config))
                        .map(|(family, _, _)| *family),
                    Some(ProviderFamily::OpenAiCompatible)
                ) {
                    self.config.set_provider("openai-compatible");
                }
                if self.config.active_openai_profile_kind() == Some(OpenAiEndpointKind::Deepseek) {
                    self.config.select_openai_profile(
                        OpenAiEndpointKind::Custom.default_profile_id(),
                        OpenAiEndpointKind::Custom.label(),
                        OpenAiEndpointKind::Custom,
                    );
                }
                self.model_picker_idx = 0;
            } else if let Some(provider) = self.single_provider_for_selected_family() {
                self.config.set_provider(provider.to_string());
                self.model_picker_idx = self.selected_preset_idx();
            }
            self.sync_reasoning_effort_picker();
        }
        if matches!(
            overlay,
            Overlay::ListPicker(ListPickerKind::OpenAiEndpointKind)
        ) {
            self.openai_endpoint_kind_picker_idx = self
                .selected_openai_profile_kind()
                .and_then(|kind| {
                    openai_profile_setup_kinds()
                        .iter()
                        .position(|candidate| *candidate == kind)
                })
                .unwrap_or(0);
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::OpenAiProfile)) {
            self.sync_openai_profile_picker();
        }
        if matches!(overlay, Overlay::BaseUrlEditor) {
            let provider_family = self.selected_provider_family();
            self.base_url_input = self.config.base_url.clone().unwrap_or_else(|| {
                if matches!(provider_family, ProviderFamily::OpenAiCompatible) {
                    self.config
                        .active_openai_profile_kind()
                        .unwrap_or(OpenAiEndpointKind::Custom)
                        .default_base_url()
                        .to_string()
                } else {
                    "http://localhost:11434".to_string()
                }
            });
            self.base_url_cursor_offset = None;
        }
        if matches!(overlay, Overlay::ApiKeyEditor(_)) {
            self.api_key_input.clear();
            self.api_key_cursor_offset = None;
        }
        if matches!(overlay, Overlay::ModelNameEditor) {
            self.model_name_input = self.config.model.clone().unwrap_or_else(|| {
                self.selected_model_preset()
                    .map(|(_, _, default_model)| default_model.to_string())
                    .unwrap_or_default()
            });
            self.model_name_cursor_offset = None;
        }
        if matches!(overlay, Overlay::OpenAiProfileLabelEditor) {
            let kind = self
                .openai_profile_label_kind
                .or_else(|| self.selected_openai_profile_kind())
                .unwrap_or(OpenAiEndpointKind::Custom);
            self.openai_profile_label_input = format!("{} profile", kind.label());
            self.openai_profile_label_cursor_offset = None;
        }
        if matches!(overlay, Overlay::ListPicker(ListPickerKind::AuthMode)) {
            self.auth_mode_idx = if is_ssh_session() { 1 } else { 0 };
        }
        if matches!(
            overlay,
            Overlay::ListPicker(ListPickerKind::ReasoningEffort)
        ) {
            self.sync_reasoning_effort_picker();
        }
        if matches!(overlay, Overlay::SkillsPicker) {
            self.skill_picker_idx = 0;
        }
        if matches!(overlay, Overlay::Context) {
            // Auto-hide the command palette when opening a full-screen modal
            // so Esc dismisses only the modal, not the stale palette underneath.
            if matches!(self.overlay, Some(Overlay::CommandPalette)) {
                self.hide_overlay();
            }
            self.context_scroll = 0;
        }
        self.overlay_stack.push(overlay);
        self.overlay = Some(overlay);
    }

    /// Pop the top overlay from the stack without user-visible side
    /// effects (preserves input, does not cancel setups). Used when the
    /// overlay becomes irrelevant due to an input change rather than an
    /// explicit user action.
    fn hide_overlay(&mut self) {
        self.overlay_stack.pop();
        self.overlay = self.overlay_stack.last().copied();
        self.command_palette_idx = 0;
    }

    /// Keep the command-palette overlay in sync with the current input.
    pub fn sync_command_palette_with_input(&mut self) {
        let should_show = input_requests_command_palette(self.bottom_pane.input.as_str());
        match (should_show, &self.overlay) {
            (true, None) => self.open_overlay(Overlay::CommandPalette),
            (false, Some(Overlay::CommandPalette)) => self.hide_overlay(),
            _ => {}
        }
    }

    pub fn dismiss_overlay(&mut self) {
        if matches!(
            self.overlay,
            Some(Overlay::BaseUrlEditor | Overlay::ApiKeyEditor(_) | Overlay::ModelNameEditor)
        ) {
            self.cancel_openai_profile_setup();
        }

        // When dismissing the command palette, clear the `/` input so
        // sync_command_palette_with_input won't immediately re-open it.
        if matches!(
            self.overlay,
            Some(Overlay::CommandPalette | Overlay::ModelSearch)
        ) {
            self.bottom_pane.input.clear();
            self.command_palette_idx = 0;
        }

        self.hide_overlay();
    }
}
