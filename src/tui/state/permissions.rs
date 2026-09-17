use std::sync::atomic::Ordering;

use super::{PermissionMode, TuiApp};
use crate::tui::permission_policy::PERMISSION_PRESETS;

impl TuiApp {
    pub(crate) fn effective_permission_mode(&self) -> PermissionMode {
        let network = self.sandbox_network_access.load(Ordering::Relaxed);
        PERMISSION_PRESETS
            .iter()
            .find(|preset| {
                preset.execution == self.agent_execution_mode
                    && preset.approval == self.bash_approval_mode
                    && preset.network == network
                    && preset.full_access == (self.permission_mode == PermissionMode::FullAccess)
            })
            .map_or(PermissionMode::Custom, |preset| preset.mode)
    }
}
