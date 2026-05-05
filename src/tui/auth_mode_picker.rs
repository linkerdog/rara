use super::command::api_key_status;
use super::state::{ProviderFamily, TuiApp};

pub(crate) struct AuthModePickerView {
    pub(crate) intro: String,
    pub(crate) lines: Vec<String>,
    pub(crate) footer: &'static str,
}

pub(crate) const AUTH_MODE_OPTION_COUNT: usize = 4;

pub(crate) fn build_auth_mode_picker_view(app: &TuiApp, ssh_session: bool) -> AuthModePickerView {
    let family = app.selected_provider_family();
    let is_gemini = family == ProviderFamily::Gemini;
    let provider_label = if is_gemini { "Gemini" } else { "Codex" };
    let provider_id: &str = if is_gemini {
        "gemini-code-assist"
    } else {
        "codex"
    };

    let ssh_hint = if ssh_session {
        "\n\nSSH session detected. Browser login on a remote shell usually cannot complete the localhost callback. Device-code login is recommended in SSH/headless sessions."
    } else {
        ""
    };
    let intro = format!(
        "{provider_label} needs authentication before this preset can be used.\n\n\
         Choose one auth mode below.{ssh_hint}"
    );
    let options: &[(&str, &str)] = if is_gemini {
        // Gemini Code Assist: browser + device-code + logout (no API key).
        // API key users should select provider=gemini for AI Studio.
        &[
            (
                "Browser login",
                "Best for local desktop sessions with a localhost callback.",
            ),
            (
                "Device code",
                "Best for SSH/headless sessions. Open the URL elsewhere and enter the one-time code.",
            ),
            (
                "—",
                "For API key access, go back and select the standard Gemini (AI Studio) provider.",
            ),
            (
                "Logout",
                "Clear the saved Google OAuth credential.",
            ),
        ]
    } else {
        &[
            (
                "Browser login",
                "Best for local desktop sessions with a localhost callback.",
            ),
            (
                "Device code",
                "Best for SSH/headless sessions. Open the URL elsewhere and enter the one-time code.",
            ),
            (
                "API key",
                "Paste an existing Codex API key and save it locally.",
            ),
            (
                "Logout",
                "Clear the saved provider credential and rebuild the current codex backend.",
            ),
        ]
    };

    debug_assert_eq!(options.len(), AUTH_MODE_OPTION_COUNT);
    let mut lines = vec![
        format!("Current model: {}", app.current_model_label()),
        format!("Provider: {provider_id}"),
        format!("Credential status: {}", api_key_status(&app.config)),
        String::new(),
    ];
    for (idx, (title, detail)) in options.iter().enumerate() {
        let marker = if idx == app.auth_mode_idx { ">" } else { " " };
        lines.push(format!("{marker} {title}"));
        lines.push(format!("    {detail}"));
    }

    let footer = if ssh_session {
        "Up/Down move  Enter choose  number keys jump  default: device code  Esc back"
    } else {
        "Up/Down move  Enter choose  number keys jump  default: browser login  Esc back"
    };

    AuthModePickerView {
        intro,
        lines,
        footer,
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use tempfile::tempdir;

    use super::build_auth_mode_picker_view;
    use crate::config::ConfigManager;
    use crate::tui::state::TuiApp;

    #[test]
    fn auth_mode_picker_local_snapshot() {
        let temp = tempdir().expect("tempdir");
        let app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");

        let popup = render_picker(&app, false);
        assert_snapshot!("auth_mode_picker_local", popup);
    }

    #[test]
    fn auth_mode_picker_ssh_snapshot() {
        let temp = tempdir().expect("tempdir");
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .expect("app");
        app.auth_mode_idx = 1;

        let popup = render_picker(&app, true);
        assert_snapshot!("auth_mode_picker_ssh", popup);
    }

    fn render_picker(app: &TuiApp, ssh_session: bool) -> String {
        let view = build_auth_mode_picker_view(app, ssh_session);
        format!(
            "{}\n\n{}\n\n{}",
            view.intro,
            view.lines.join("\n"),
            view.footer
        )
    }
}
