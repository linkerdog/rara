use std::sync::OnceLock;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalInfo {
    pub name: TerminalName,
    pub term_program: Option<String>,
    pub version: Option<String>,
    pub term: Option<String>,
    pub multiplexer: Option<Multiplexer>,
    pub remote: Option<RemoteSession>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalName {
    AppleTerminal,
    Ghostty,
    Iterm2,
    WarpTerminal,
    VsCode,
    WezTerm,
    Kitty,
    Alacritty,
    Konsole,
    GnomeTerminal,
    Vte,
    WindowsTerminal,
    Dumb,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Multiplexer {
    Tmux { version: Option<String> },
    Zellij,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteSession {
    Ssh,
}

impl TerminalInfo {
    fn new(
        name: TerminalName,
        term_program: Option<String>,
        version: Option<String>,
        term: Option<String>,
        multiplexer: Option<Multiplexer>,
        remote: Option<RemoteSession>,
    ) -> Self {
        Self {
            name,
            term_program,
            version,
            term,
            multiplexer,
            remote,
        }
    }

    fn from_term_program(
        name: TerminalName,
        term_program: String,
        version: Option<String>,
        multiplexer: Option<Multiplexer>,
        remote: Option<RemoteSession>,
    ) -> Self {
        Self::new(name, Some(term_program), version, None, multiplexer, remote)
    }

    fn from_name(
        name: TerminalName,
        version: Option<String>,
        multiplexer: Option<Multiplexer>,
        remote: Option<RemoteSession>,
    ) -> Self {
        Self::new(name, None, version, None, multiplexer, remote)
    }

    fn from_term(
        term: String,
        multiplexer: Option<Multiplexer>,
        remote: Option<RemoteSession>,
    ) -> Self {
        let name = match term.as_str() {
            "dumb" => TerminalName::Dumb,
            "wezterm" | "wezterm-mux" => TerminalName::WezTerm,
            value if value.contains("kitty") => TerminalName::Kitty,
            "alacritty" => TerminalName::Alacritty,
            _ => TerminalName::Unknown,
        };
        Self::new(name, None, None, Some(term), multiplexer, remote)
    }

    fn unknown(multiplexer: Option<Multiplexer>, remote: Option<RemoteSession>) -> Self {
        Self::new(TerminalName::Unknown, None, None, None, multiplexer, remote)
    }

    pub fn is_remote_session(&self) -> bool {
        self.remote.is_some()
    }

    pub fn is_zellij(&self) -> bool {
        matches!(self.multiplexer, Some(Multiplexer::Zellij))
    }

    pub fn user_agent_token(&self) -> String {
        let raw = if let Some(program) = self.term_program.as_ref() {
            match self.version.as_ref().filter(|value| !value.is_empty()) {
                Some(version) => format!("{program}/{version}"),
                None => program.clone(),
            }
        } else if let Some(term) = self.term.as_ref().filter(|value| !value.is_empty()) {
            term.clone()
        } else {
            match self.name {
                TerminalName::AppleTerminal => {
                    format_terminal_version("Apple_Terminal", &self.version)
                }
                TerminalName::Ghostty => format_terminal_version("Ghostty", &self.version),
                TerminalName::Iterm2 => format_terminal_version("iTerm.app", &self.version),
                TerminalName::WarpTerminal => {
                    format_terminal_version("WarpTerminal", &self.version)
                }
                TerminalName::VsCode => format_terminal_version("vscode", &self.version),
                TerminalName::WezTerm => format_terminal_version("WezTerm", &self.version),
                TerminalName::Kitty => "kitty".to_string(),
                TerminalName::Alacritty => "Alacritty".to_string(),
                TerminalName::Konsole => format_terminal_version("Konsole", &self.version),
                TerminalName::GnomeTerminal => "gnome-terminal".to_string(),
                TerminalName::Vte => format_terminal_version("VTE", &self.version),
                TerminalName::WindowsTerminal => "WindowsTerminal".to_string(),
                TerminalName::Dumb => "dumb".to_string(),
                TerminalName::Unknown => "unknown".to_string(),
            }
        };
        sanitize_header_value(raw)
    }
}

static TERMINAL_INFO: OnceLock<TerminalInfo> = OnceLock::new();

trait Environment {
    fn var(&self, name: &str) -> Option<String>;

    fn has(&self, name: &str) -> bool {
        self.var(name).is_some()
    }

    fn has_non_empty(&self, name: &str) -> bool {
        self.var_non_empty(name).is_some()
    }

    fn var_non_empty(&self, name: &str) -> Option<String> {
        self.var(name).and_then(none_if_whitespace)
    }
}

struct ProcessEnvironment;

impl Environment for ProcessEnvironment {
    fn var(&self, name: &str) -> Option<String> {
        match std::env::var(name) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => None,
        }
    }
}

pub fn terminal_info() -> TerminalInfo {
    TERMINAL_INFO
        .get_or_init(|| detect_terminal_info_from_env(&ProcessEnvironment))
        .clone()
}

pub fn is_remote_session() -> bool {
    terminal_info().is_remote_session()
}

fn detect_terminal_info_from_env(env: &dyn Environment) -> TerminalInfo {
    let multiplexer = detect_multiplexer(env);
    let remote = detect_remote_session(env);

    if let Some(term_program) = env.var_non_empty("TERM_PROGRAM") {
        let version = env.var_non_empty("TERM_PROGRAM_VERSION");
        let name = terminal_name_from_term_program(&term_program).unwrap_or(TerminalName::Unknown);
        return TerminalInfo::from_term_program(name, term_program, version, multiplexer, remote);
    }

    if env.has("WEZTERM_VERSION") {
        return TerminalInfo::from_name(
            TerminalName::WezTerm,
            env.var_non_empty("WEZTERM_VERSION"),
            multiplexer,
            remote,
        );
    }

    if env.has("ITERM_SESSION_ID") || env.has("ITERM_PROFILE") || env.has("ITERM_PROFILE_NAME") {
        return TerminalInfo::from_name(TerminalName::Iterm2, None, multiplexer, remote);
    }

    if env.has("TERM_SESSION_ID") {
        return TerminalInfo::from_name(TerminalName::AppleTerminal, None, multiplexer, remote);
    }

    if env.has("KITTY_WINDOW_ID") || env.var("TERM").is_some_and(|term| term.contains("kitty")) {
        return TerminalInfo::from_name(TerminalName::Kitty, None, multiplexer, remote);
    }

    if env.has("ALACRITTY_SOCKET") || env.var("TERM").is_some_and(|term| term == "alacritty") {
        return TerminalInfo::from_name(TerminalName::Alacritty, None, multiplexer, remote);
    }

    if env.has("KONSOLE_VERSION") {
        return TerminalInfo::from_name(
            TerminalName::Konsole,
            env.var_non_empty("KONSOLE_VERSION"),
            multiplexer,
            remote,
        );
    }

    if env.has("GNOME_TERMINAL_SCREEN") {
        return TerminalInfo::from_name(TerminalName::GnomeTerminal, None, multiplexer, remote);
    }

    if env.has("VTE_VERSION") {
        return TerminalInfo::from_name(
            TerminalName::Vte,
            env.var_non_empty("VTE_VERSION"),
            multiplexer,
            remote,
        );
    }

    if env.has("WT_SESSION") {
        return TerminalInfo::from_name(TerminalName::WindowsTerminal, None, multiplexer, remote);
    }

    if let Some(term) = env.var_non_empty("TERM") {
        return TerminalInfo::from_term(term, multiplexer, remote);
    }

    TerminalInfo::unknown(multiplexer, remote)
}

fn detect_multiplexer(env: &dyn Environment) -> Option<Multiplexer> {
    if env.has_non_empty("TMUX") || env.has_non_empty("TMUX_PANE") {
        return Some(Multiplexer::Tmux {
            version: tmux_version_from_env(env),
        });
    }

    if env.has_non_empty("ZELLIJ")
        || env.has_non_empty("ZELLIJ_SESSION_NAME")
        || env.has_non_empty("ZELLIJ_VERSION")
    {
        return Some(Multiplexer::Zellij);
    }

    None
}

fn detect_remote_session(env: &dyn Environment) -> Option<RemoteSession> {
    (env.has_non_empty("SSH_CONNECTION") || env.has_non_empty("SSH_TTY"))
        .then_some(RemoteSession::Ssh)
}

fn tmux_version_from_env(env: &dyn Environment) -> Option<String> {
    env.var("TERM_PROGRAM")
        .filter(|value| value.eq_ignore_ascii_case("tmux"))
        .and_then(|_| env.var_non_empty("TERM_PROGRAM_VERSION"))
}

fn terminal_name_from_term_program(value: &str) -> Option<TerminalName> {
    let normalized = value
        .trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_' | '.'))
        .map(|c| c.to_ascii_lowercase())
        .collect::<String>();

    match normalized.as_str() {
        "appleterminal" => Some(TerminalName::AppleTerminal),
        "ghostty" => Some(TerminalName::Ghostty),
        "iterm" | "iterm2" | "itermapp" => Some(TerminalName::Iterm2),
        "warp" | "warpterminal" => Some(TerminalName::WarpTerminal),
        "vscode" => Some(TerminalName::VsCode),
        "wezterm" => Some(TerminalName::WezTerm),
        "kitty" => Some(TerminalName::Kitty),
        "alacritty" => Some(TerminalName::Alacritty),
        "konsole" => Some(TerminalName::Konsole),
        "gnometerminal" => Some(TerminalName::GnomeTerminal),
        "vte" => Some(TerminalName::Vte),
        "windowsterminal" => Some(TerminalName::WindowsTerminal),
        "dumb" => Some(TerminalName::Dumb),
        _ => None,
    }
}

fn format_terminal_version(name: &str, version: &Option<String>) -> String {
    match version.as_ref().filter(|value| !value.is_empty()) {
        Some(version) => format!("{name}/{version}"),
        None => name.to_string(),
    }
}

fn sanitize_header_value(value: String) -> String {
    value.replace(|c| !is_valid_header_value_char(c), "_")
}

fn is_valid_header_value_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/'
}

fn none_if_whitespace(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct FakeEnvironment {
        vars: HashMap<String, String>,
    }

    impl FakeEnvironment {
        fn with_var(mut self, key: &str, value: &str) -> Self {
            self.vars.insert(key.to_string(), value.to_string());
            self
        }
    }

    impl Environment for FakeEnvironment {
        fn var(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned()
        }
    }

    #[test]
    fn detects_term_program_with_version() {
        let env = FakeEnvironment::default()
            .with_var("TERM_PROGRAM", "iTerm.app")
            .with_var("TERM_PROGRAM_VERSION", "3.5.0");
        let info = detect_terminal_info_from_env(&env);

        assert_eq!(info.name, TerminalName::Iterm2);
        assert_eq!(info.term_program.as_deref(), Some("iTerm.app"));
        assert_eq!(info.version.as_deref(), Some("3.5.0"));
        assert_eq!(info.user_agent_token(), "iTerm.app/3.5.0");
    }

    #[test]
    fn detects_terminal_specific_variables() {
        let wezterm = detect_terminal_info_from_env(
            &FakeEnvironment::default().with_var("WEZTERM_VERSION", "2024.2"),
        );
        assert_eq!(wezterm.name, TerminalName::WezTerm);
        assert_eq!(wezterm.version.as_deref(), Some("2024.2"));

        let kitty = detect_terminal_info_from_env(
            &FakeEnvironment::default().with_var("KITTY_WINDOW_ID", "1"),
        );
        assert_eq!(kitty.name, TerminalName::Kitty);
    }

    #[test]
    fn detects_remote_and_mux_markers() {
        let info = detect_terminal_info_from_env(
            &FakeEnvironment::default()
                .with_var("SSH_CONNECTION", "1.2.3.4")
                .with_var("ZELLIJ_SESSION_NAME", "main")
                .with_var("TERM", "xterm-256color"),
        );

        assert_eq!(info.remote, Some(RemoteSession::Ssh));
        assert_eq!(info.multiplexer, Some(Multiplexer::Zellij));
        assert!(info.is_remote_session());
        assert!(info.is_zellij());
    }

    #[test]
    fn detects_tmux_version_when_term_program_is_tmux() {
        let info = detect_terminal_info_from_env(
            &FakeEnvironment::default()
                .with_var("TMUX", "/tmp/tmux")
                .with_var("TERM_PROGRAM", "tmux")
                .with_var("TERM_PROGRAM_VERSION", "3.4"),
        );

        assert_eq!(
            info.multiplexer,
            Some(Multiplexer::Tmux {
                version: Some("3.4".to_string())
            })
        );
    }

    #[test]
    fn falls_back_to_term() {
        let info =
            detect_terminal_info_from_env(&FakeEnvironment::default().with_var("TERM", "dumb"));

        assert_eq!(info.name, TerminalName::Dumb);
        assert_eq!(info.term.as_deref(), Some("dumb"));
        assert_eq!(info.user_agent_token(), "dumb");
    }

    #[test]
    fn sanitizes_user_agent_token() {
        let info = detect_terminal_info_from_env(
            &FakeEnvironment::default()
                .with_var("TERM_PROGRAM", "Bad Terminal")
                .with_var("TERM_PROGRAM_VERSION", "1.0\nbad"),
        );

        assert_eq!(info.user_agent_token(), "Bad_Terminal/1.0_bad");
    }
}
