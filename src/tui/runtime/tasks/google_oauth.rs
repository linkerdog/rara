use std::sync::Arc;

use anyhow::anyhow;
use tokio::sync::mpsc;

use crate::google_oauth::{GoogleCredential, GoogleOAuthManager};
use crate::tui::state::{
    OAuthLoginMode, RunningTask, RuntimePhase, TaskCompletion, TaskKind, TuiApp, TuiEvent,
};

/// Reserved for Gemini Code Assist OAuth connection from the TUI (docs/todo.md).
#[allow(dead_code)]
pub(crate) fn start_google_oauth_task(
    app: &mut TuiApp,
    oauth_manager: Arc<GoogleOAuthManager>,
    mode: OAuthLoginMode,
) {
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
    app.bottom_pane.notice = Some(format!("Starting Google {mode_label}."));
    app.set_runtime_phase(
        RuntimePhase::OAuthStarting,
        Some(format!("starting google {mode_label}")),
    );
    app.push_entry("Runtime", format!("Starting Google {mode_label} flow."));

    let handle = tokio::spawn(async move {
        let result = run_google_oauth_login(oauth_manager, mode, sender.clone()).await;
        TaskCompletion::GoogleOAuth { mode, result }
    });

    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::GoogleOAuth,
        receiver,
        handle,
        started_at: std::time::Instant::now(),
        next_heartbeat_after_secs: u64::MAX,
        cancellation_token: None,
        cancellation_requested: false,
    });
}

/// Reserved for Gemini Code Assist OAuth connection from the TUI (docs/todo.md).
#[allow(dead_code)]
pub(super) async fn run_google_oauth_login(
    oauth_manager: Arc<GoogleOAuthManager>,
    mode: OAuthLoginMode,
    sender: mpsc::UnboundedSender<TuiEvent>,
) -> anyhow::Result<GoogleCredential> {
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
            let session = oauth_manager.start_browser_login()?;
            let _ = sender.send(TuiEvent::Transcript {
                role: "Runtime",
                message: format!(
                    "Starting Google browser login.\nOpen this URL if the browser does not launch automatically:\n{auth_url}",
                    auth_url = session.auth_url
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
            GoogleOAuthManager::complete_browser_login(session).await
        }
        OAuthLoginMode::DeviceCode => {
            let _ = sender.send(TuiEvent::Transcript {
                role: "Runtime",
                message: "Requesting Google device code.".into(),
            });
            let device_code = oauth_manager.request_device_code().await?;
            let _ = sender.send(TuiEvent::Transcript {
                role: "Runtime",
                message: format!(
                    "Open {} in a browser and enter this code: {}\nWaiting for authorization...",
                    device_code.verification_url, device_code.user_code
                ),
            });
            GoogleOAuthManager::complete_device_code(device_code).await
        }
    }
}
