use std::sync::Arc;

use anyhow::anyhow;
use secrecy::SecretString;
use tokio::sync::mpsc;

use crate::oauth::OAuthManager;
use crate::tui::state::{
    OAuthLoginMode, RunningTask, RuntimePhase, TaskCompletion, TaskKind, TuiApp, TuiEvent,
};

pub(crate) fn start_oauth_task(
    app: &mut TuiApp,
    oauth_manager: Arc<OAuthManager>,
    mode: OAuthLoginMode,
) {
    oauth_manager.invalidate_saved_auth_cache();
    if matches!(mode, OAuthLoginMode::Browser) && super::super::super::is_ssh_session() {
        app.set_runtime_phase(
            RuntimePhase::Failed,
            Some("browser oauth unavailable in ssh".into()),
        );
        app.push_notice(
            "Browser login is unavailable in SSH/headless sessions. Choose device code or API key instead.",
        );
        app.push_entry(
            "Runtime",
            "Browser login is unavailable in SSH/headless sessions. Use device-code login or API key instead.",
        );
        return;
    }

    let (sender, receiver) = mpsc::unbounded_channel();
    let mode_label = match mode {
        OAuthLoginMode::Browser => "browser login",
        OAuthLoginMode::DeviceCode => "device-code login",
    };
    app.bottom_pane.notice = Some(format!("Starting Codex {mode_label}."));
    app.set_runtime_phase(
        RuntimePhase::OAuthStarting,
        Some(format!("starting {mode_label}")),
    );
    app.push_entry("Runtime", format!("Starting Codex {mode_label} flow."));

    let handle = tokio::spawn(async move {
        let result = run_oauth_login(oauth_manager, mode, sender.clone()).await;
        TaskCompletion::OAuth { mode, result }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::OAuth,
        receiver,
        handle,
        started_at: std::time::Instant::now(),
        next_heartbeat_after_secs: u64::MAX,
        cancellation_token: None,
        cancellation_requested: false,
    });
}

pub(super) async fn run_oauth_login(
    oauth_manager: Arc<OAuthManager>,
    mode: OAuthLoginMode,
    sender: mpsc::UnboundedSender<TuiEvent>,
) -> anyhow::Result<SecretString> {
    match mode {
        OAuthLoginMode::Browser => {
            let is_ssh = super::super::super::is_ssh_session();
            if is_ssh {
                let _ = sender.send(TuiEvent::Transcript {
                    role: "Runtime",
                    message: "SSH session detected. Browser login is unavailable because the callback listens on localhost.\nUse device-code login or API key instead."
                        .into(),
                });
                return Err(anyhow!(
                    "browser login is unavailable in SSH/headless sessions; use device-code login or API key instead"
                ));
            }
            let session = oauth_manager.start_browser_login(true)?;
            let _ = sender.send(TuiEvent::Transcript {
                role: "Runtime",
                message: format!(
                    "Starting Codex browser login.\nOpen this URL if the browser does not launch automatically:\n{auth_url}",
                    auth_url = session.auth_url()
                ),
            });
            let _ = sender.send(TuiEvent::Transcript {
                role: "Runtime",
                message: "Waiting for browser callback.".into(),
            });
            let _ = sender.send(TuiEvent::Transcript {
                role: "Runtime",
                message: "Received browser callback, exchanging token.".into(),
            });
            session.complete(&oauth_manager).await
        }
        OAuthLoginMode::DeviceCode => {
            let _ = sender.send(TuiEvent::Transcript {
                role: "Runtime",
                message: "Requesting Codex device code from OpenAI.".into(),
            });
            let device_code = oauth_manager.request_device_code().await?;
            let _ = sender.send(TuiEvent::Transcript {
                role: "Runtime",
                message: format!(
                    "Open this URL in a browser and enter the one-time code:\n{}\n\nCode: {}",
                    device_code.verification_url, device_code.user_code
                ),
            });
            let _ = sender.send(TuiEvent::Transcript {
                role: "Runtime",
                message: "Waiting for device-code confirmation.".into(),
            });
            oauth_manager.complete_device_code_login(&device_code).await
        }
    }
}
