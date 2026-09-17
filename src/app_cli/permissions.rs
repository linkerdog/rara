use super::{Cli, Commands};
use crate::runtime_context::RuntimeBootstrap;
use crate::runtime_session::RuntimeSession;
use crate::tui::state::PermissionMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartupPermissions {
    Default,
    FullAccess,
}

impl StartupPermissions {
    pub(super) fn from_cli(cli: &Cli) -> anyhow::Result<Self> {
        if !cli.dangerously_skip_permissions {
            return Ok(Self::Default);
        }
        match &cli.command {
            None
            | Some(
                Commands::Tui
                | Commands::Resume { .. }
                | Commands::Ask { .. }
                | Commands::Print { .. }
                | Commands::Wire { .. }
                | Commands::Exec(_)
                | Commands::AppServer(_),
            ) => Ok(Self::FullAccess),
            Some(
                Commands::Acp
                | Commands::Connect(_)
                | Commands::Models(_)
                | Commands::Plugin(_)
                | Commands::Mem(_)
                | Commands::Fork { .. }
                | Commands::Distill { .. }
                | Commands::Thread { .. }
                | Commands::Threads { .. }
                | Commands::Login { .. }
                | Commands::Logout,
            ) => anyhow::bail!(
                "--dangerously-skip-permissions is supported by tui, resume, ask, print, wire, exec, and app-server"
            ),
        }
    }

    pub(super) fn tui_override(self) -> Option<PermissionMode> {
        match self {
            Self::Default => None,
            Self::FullAccess => Some(PermissionMode::FullAccess),
        }
    }

    pub(super) async fn start_headless_session(
        self,
        bootstrap: RuntimeBootstrap,
    ) -> anyhow::Result<RuntimeSession> {
        if self == Self::FullAccess {
            bootstrap
                .sandbox_network_access
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let session = RuntimeSession::from_bootstrap(bootstrap).await?;
        if self == Self::FullAccess {
            session.set_full_access_mode(true).await?;
        }
        Ok(session)
    }
}
