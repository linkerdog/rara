use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use uuid::Uuid;

pub struct SandboxManager {
    os: String,
    profile_dir: PathBuf,
    sandbox_home: PathBuf,
    backend: SandboxBackend,
    command_install_roots: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct WrappedCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cleanup_path: Option<PathBuf>,
    pub sandboxed: bool,
    pub sandbox_backend: String,
    pub sandbox_home: Option<PathBuf>,
    pub network_access: bool,
}

const LINUX_RUNTIME_READ_ROOTS: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr",
    "/opt",
    "/etc",
    "/lib",
    "/lib64",
    "/nix/store",
    "/run/current-system/sw",
];
const MACOS_SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
const DEFAULT_SHELL: &str = "/bin/sh";
const PROFILE_CLEANUP_AGE: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
enum SandboxBackend {
    MacosSeatbelt,
    LinuxBubblewrap,
    Direct,
    Unsupported { platform: String },
}

impl SandboxManager {
    pub fn new() -> Result<Self> {
        let os = std::env::consts::OS.to_string();
        let root = std::env::current_dir()?;
        let rara_dir = rara_config::workspace_data_dir_for(&root)?;
        Self::new_with_rara_dir(os, rara_dir)
    }

    pub fn new_for_rara_dir(rara_dir: PathBuf) -> Result<Self> {
        let os = std::env::consts::OS.to_string();
        Self::new_with_rara_dir(os, rara_dir)
    }

    pub fn new_with_command_path(command_path: Option<String>) -> Result<Self> {
        let os = std::env::consts::OS.to_string();
        let root = std::env::current_dir()?;
        let rara_dir = rara_config::workspace_data_dir_for(&root)?;
        Self::new_with_rara_dir_and_command_path(os, rara_dir, command_path.map(OsString::from))
    }

    fn new_with_rara_dir(os: String, rara_dir: PathBuf) -> Result<Self> {
        Self::new_with_rara_dir_and_command_path(os, rara_dir, env::var_os("PATH"))
    }

    fn new_with_rara_dir_and_command_path(
        os: String,
        rara_dir: PathBuf,
        command_path: Option<OsString>,
    ) -> Result<Self> {
        if !rara_dir.exists() {
            fs::create_dir_all(&rara_dir)?;
        }
        let profile_dir = rara_dir.join("sandbox");
        if !profile_dir.exists() {
            fs::create_dir_all(&profile_dir)?;
        }
        cleanup_stale_profiles(&profile_dir)?;
        let sandbox_home = process_sandbox_home();

        let backend = SandboxBackend::detect(os.as_str());
        let command_install_roots = command_search_install_roots(command_path.as_deref());

        Ok(Self {
            os,
            profile_dir,
            sandbox_home,
            backend,
            command_install_roots,
        })
    }

    fn create_profile(&self, allow_net: bool) -> Result<PathBuf> {
        if self.os != "macos" {
            bail!("sandbox profile creation is only supported on macOS");
        }

        let mut file_rules = String::new();
        if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
            for sensitive_dir in [".ssh", ".aws"] {
                let path = home.join(sensitive_dir);
                file_rules.push_str(&format!(
                    "(deny file-read* (subpath \"{}\"))\n",
                    sandbox_profile_string_literal(&path)
                ));
            }
        }
        for root in &self.command_install_roots {
            let root = sandbox_profile_string_literal(root);
            file_rules.push_str(&format!(
                "(allow file-read* (subpath \"{root}\"))\n(allow file-map-executable (subpath \"{root}\"))\n"
            ));
        }

        let mut net_rules = String::new();
        if allow_net {
            net_rules.push_str("(allow network*)");
        } else {
            net_rules.push_str("(deny network*)\n(allow network-outbound (literal \"/private/var/run/mDNSResponder\"))\n");
        }

        let profile = format!(
            r#"(version 1)
(deny default)
{}
(allow file-read* (subpath "/usr/bin"))
(allow file-read* (subpath "/bin"))
(allow file-read* (subpath "/System"))
(allow file-read* (subpath (param "CWD")))
(allow file-write* (subpath (param "CWD")))
(allow file-write* (subpath "/tmp"))
(allow process*)
(allow sysctl-read)
{}
"#,
            file_rules, net_rules
        );
        let profile_path = self
            .profile_dir
            .join(format!("sandbox-{}.sb", Uuid::new_v4()));
        fs::write(&profile_path, profile)?;
        Ok(profile_path)
    }

    fn append_mount_target_parent_dirs(args: &mut Vec<String>, mount_target: &Path) {
        let Some(mount_target_dir) = mount_target.parent() else {
            return;
        };

        let mut mount_target_dirs: Vec<PathBuf> = mount_target_dir
            .ancestors()
            .take_while(|path| path != &Path::new("/"))
            .map(Path::to_path_buf)
            .collect();
        mount_target_dirs.reverse();
        for mount_target_dir in mount_target_dirs {
            args.push("--dir".to_string());
            args.push(mount_target_dir.display().to_string());
        }
    }

    fn append_ro_bind(args: &mut Vec<String>, path: &Path) {
        Self::append_mount_target_parent_dirs(args, path);
        args.push("--ro-bind".to_string());
        args.push(path.display().to_string());
        args.push(path.display().to_string());
    }

    fn linux_runtime_read_roots(&self) -> Vec<PathBuf> {
        let mut roots = LINUX_RUNTIME_READ_ROOTS
            .iter()
            .map(PathBuf::from)
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        for path_dir in &self.command_install_roots {
            if roots.iter().any(|root| path_dir.starts_with(root)) {
                continue;
            }
            roots.push(path_dir.clone());
        }
        roots
    }

    fn linux_sandbox_args(&self, cwd: &str, allow_net: bool) -> Vec<String> {
        let cwd_path = Path::new(cwd);
        let mut args = vec![
            "--new-session".to_string(),
            "--die-with-parent".to_string(),
            "--tmpfs".to_string(),
            "/".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
            // Mount host /dev/pts so that programs (e.g. Python os.ttyname)
            // can find their controlling terminal inside the sandbox.
            // bubblewrap --dev only creates static nodes (null, zero, …);
            // it does not propagate the PTY slave from the parent terminal.
            "--dev-bind-try".to_string(),
            "/dev/pts".to_string(),
            "/dev/pts".to_string(),
            "--proc".to_string(),
            "--tmpfs".to_string(),
            "/tmp".to_string(),
            "--dir".to_string(),
            "/run".to_string(),
            "--dir".to_string(),
            "/var".to_string(),
            "--symlink".to_string(),
            "../run".to_string(),
            "/var/run".to_string(),
        ];
        for dir in sandbox_home_dirs(&self.sandbox_home) {
            args.push("--dir".to_string());
            args.push(dir.display().to_string());
        }

        for root in self.linux_runtime_read_roots() {
            Self::append_ro_bind(&mut args, &root);
        }

        if let Ok(resolv_conf) = fs::metadata("/etc/resolv.conf")
            && resolv_conf.is_file()
        {
            Self::append_ro_bind(&mut args, Path::new("/etc/resolv.conf"));
        }

        Self::append_mount_target_parent_dirs(&mut args, cwd_path);
        args.push("--bind".to_string());
        args.push(cwd.to_string());
        args.push(cwd.to_string());
        args.push("--chdir".to_string());
        args.push(cwd.to_string());

        if !allow_net {
            args.push("--unshare-net".to_string());
        }

        args
    }

    pub fn wrap_shell_command(
        &self,
        original_cmd: &str,
        cwd: &str,
        allow_net: bool,
    ) -> Result<WrappedCommand> {
        let shell = shell_program();
        let shell_flag = shell_command_flag(&shell, false);
        match &self.backend {
            SandboxBackend::MacosSeatbelt => {
                let profile_path = self.create_profile(allow_net)?;
                Ok(WrappedCommand {
                    program: MACOS_SANDBOX_EXEC.to_string(),
                    args: vec![
                        "-D".to_string(),
                        format!("CWD={cwd}"),
                        "-f".to_string(),
                        profile_path.display().to_string(),
                        shell,
                        shell_flag,
                        original_cmd.to_string(),
                    ],
                    cleanup_path: Some(profile_path),
                    sandboxed: true,
                    sandbox_backend: self.backend.name().to_string(),
                    sandbox_home: Some(self.sandbox_home.clone()),
                    network_access: allow_net,
                })
            }
            SandboxBackend::LinuxBubblewrap => {
                let mut args = self.linux_sandbox_args(cwd, allow_net);
                args.push("--".to_string());
                args.push(shell);
                args.push(shell_flag);
                args.push(original_cmd.to_string());
                Ok(WrappedCommand {
                    program: "bwrap".to_string(),
                    args,
                    cleanup_path: None,
                    sandboxed: true,
                    sandbox_backend: self.backend.name().to_string(),
                    sandbox_home: Some(self.sandbox_home.clone()),
                    network_access: allow_net,
                })
            }
            SandboxBackend::Direct => Ok(wrap_direct_shell_command(original_cmd)),
            SandboxBackend::Unsupported { platform } => bail!(
                "sandboxed command execution is unsupported on platform {}",
                platform
            ),
        }
    }

    pub fn wrap_unsandboxed_shell_command(&self, original_cmd: &str) -> WrappedCommand {
        wrap_direct_shell_command(original_cmd)
    }

    pub fn wrap_pty_shell_command(
        &self,
        original_cmd: &str,
        cwd: &str,
        allow_net: bool,
    ) -> Result<WrappedCommand> {
        match &self.backend {
            // macOS sandbox-exec does not preserve interactive PTY stdin reliably.
            SandboxBackend::MacosSeatbelt => Ok(wrap_direct_shell_command(original_cmd)),
            _ => self.wrap_shell_command(original_cmd, cwd, allow_net),
        }
    }

    pub fn wrap_pty_exec_command(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
        allow_net: bool,
    ) -> Result<WrappedCommand> {
        match &self.backend {
            // macOS sandbox-exec does not preserve interactive PTY stdin reliably.
            SandboxBackend::MacosSeatbelt => Ok(wrap_direct_exec_command(program, args)),
            _ => self.wrap_exec_command(program, args, cwd, allow_net),
        }
    }

    pub fn wrap_unsandboxed_exec_command(&self, program: &str, args: &[String]) -> WrappedCommand {
        wrap_direct_exec_command(program, args)
    }

    pub fn wrap_exec_command(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
        allow_net: bool,
    ) -> Result<WrappedCommand> {
        match &self.backend {
            SandboxBackend::MacosSeatbelt => {
                let profile_path = self.create_profile(allow_net)?;
                let mut wrapped_args = vec![
                    "-D".to_string(),
                    format!("CWD={cwd}"),
                    "-f".to_string(),
                    profile_path.display().to_string(),
                    program.to_string(),
                ];
                wrapped_args.extend(args.iter().cloned());
                Ok(WrappedCommand {
                    program: MACOS_SANDBOX_EXEC.to_string(),
                    args: wrapped_args,
                    cleanup_path: Some(profile_path),
                    sandboxed: true,
                    sandbox_backend: self.backend.name().to_string(),
                    sandbox_home: Some(self.sandbox_home.clone()),
                    network_access: allow_net,
                })
            }
            SandboxBackend::LinuxBubblewrap => {
                let mut wrapped_args = self.linux_sandbox_args(cwd, allow_net);
                wrapped_args.push("--".to_string());
                wrapped_args.push(program.to_string());
                wrapped_args.extend(args.iter().cloned());
                Ok(WrappedCommand {
                    program: "bwrap".to_string(),
                    args: wrapped_args,
                    cleanup_path: None,
                    sandboxed: true,
                    sandbox_backend: self.backend.name().to_string(),
                    sandbox_home: Some(self.sandbox_home.clone()),
                    network_access: allow_net,
                })
            }
            SandboxBackend::Direct => Ok(wrap_direct_exec_command(program, args)),
            SandboxBackend::Unsupported { platform } => bail!(
                "sandboxed command execution is unsupported on platform {}",
                platform
            ),
        }
    }

    pub fn explain_violation(&self, stderr: &str) -> Option<String> {
        if stderr.contains("Operation not permitted") || stderr.contains("Sandbox: Violation") {
            Some("Blocked by RARA Sandbox.".into())
        } else {
            None
        }
    }
}

pub fn sandbox_failure_hint() -> &'static str {
    "Sandboxed bash could not complete this command. Prefer direct file tools such as read_file, apply_patch, and replace_lines; if shell access is required, ask the user to run or approve a shell-specific path."
}

impl SandboxBackend {
    fn detect(os: &str) -> Self {
        match os {
            "macos" if Path::new(MACOS_SANDBOX_EXEC).is_file() => Self::MacosSeatbelt,
            "macos" => Self::Unsupported {
                platform: format!(
                    "macos (sandbox unavailable: {} is missing)",
                    MACOS_SANDBOX_EXEC
                ),
            },
            "linux" if command_exists("bwrap") => Self::LinuxBubblewrap,
            "linux" => Self::Unsupported {
                platform: "linux (sandbox unavailable: install bubblewrap/bwrap)".to_string(),
            },
            platform => Self::Unsupported {
                platform: platform.to_string(),
            },
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::MacosSeatbelt => "macos-seatbelt",
            Self::LinuxBubblewrap => "linux-bubblewrap",
            Self::Direct => "direct",
            Self::Unsupported { .. } => "unsupported",
        }
    }
}

fn wrap_direct_shell_command(original_cmd: &str) -> WrappedCommand {
    let shell = shell_program();
    let shell_flag = shell_command_flag(&shell, false);
    WrappedCommand {
        program: shell,
        args: vec![shell_flag, original_cmd.to_string()],
        cleanup_path: None,
        sandboxed: false,
        sandbox_backend: SandboxBackend::Direct.name().to_string(),
        sandbox_home: None,
        network_access: true,
    }
}

fn wrap_direct_exec_command(program: &str, args: &[String]) -> WrappedCommand {
    WrappedCommand {
        program: program.to_string(),
        args: args.to_vec(),
        cleanup_path: None,
        sandboxed: false,
        sandbox_backend: SandboxBackend::Direct.name().to_string(),
        sandbox_home: None,
        network_access: true,
    }
}

fn command_exists(program: &str) -> bool {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return program_path.exists();
    }

    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

fn command_search_path_dirs(command_path: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    let home_dir = env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|path| fs::canonicalize(&path).ok().or(Some(path)));
    command_path
        .map(OsString::from)
        .or_else(|| env::var_os("PATH"))
        .map(|paths| {
            let mut dirs = Vec::new();
            for dir in env::split_paths(&paths) {
                if !dir.is_absolute() || !dir.is_dir() {
                    continue;
                }
                if path_contains_control_chars(&dir) {
                    continue;
                }
                let dir = fs::canonicalize(&dir).unwrap_or(dir);
                if path_contains_control_chars(&dir) {
                    continue;
                }
                if is_broad_command_search_dir(&dir, home_dir.as_deref()) {
                    continue;
                }
                if !dirs.iter().any(|existing| existing == &dir) {
                    dirs.push(dir);
                }
            }
            dirs
        })
        .unwrap_or_default()
}

fn is_broad_command_search_dir(dir: &Path, home_dir: Option<&Path>) -> bool {
    dir == Path::new("/") || home_dir == Some(dir)
}

fn command_search_install_roots(command_path: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let home_dir = env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|path| fs::canonicalize(&path).ok().or(Some(path)));
    for dir in command_search_path_dirs(command_path) {
        let root = if matches!(
            dir.file_name().and_then(|name| name.to_str()),
            Some("bin" | "sbin")
        ) {
            let parent = dir.parent().unwrap_or(dir.as_path());
            if parent == Path::new("/") || home_dir.as_deref() == Some(parent) {
                dir.clone()
            } else {
                parent.to_path_buf()
            }
        } else {
            dir
        };
        if !roots.iter().any(|existing| existing == &root) {
            roots.push(root);
        }
    }
    roots
}

fn path_contains_control_chars(path: &Path) -> bool {
    path.display().to_string().chars().any(char::is_control)
}

fn sandbox_profile_string_literal(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn shell_program() -> String {
    env::var("SHELL")
        .ok()
        .and_then(|value| sanitize_shell_program(value.as_str()))
        .unwrap_or_else(|| DEFAULT_SHELL.to_string())
}

fn sanitize_shell_program(value: &str) -> Option<String> {
    let shell = value.split_whitespace().next()?.trim();
    if matches!(
        shell,
        "/bin/sh"
            | "/bin/bash"
            | "/bin/zsh"
            | "/bin/ksh"
            | "/usr/bin/sh"
            | "/usr/bin/bash"
            | "/usr/bin/zsh"
            | "/usr/bin/ksh"
    ) {
        Some(shell.to_string())
    } else {
        None
    }
}

fn shell_command_flag(shell: &str, use_login_shell: bool) -> String {
    let name = Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shell);
    // Use -c (non-login) so shell init files are not sourced.
    // When use_login_shell is false, we avoid sourcing .zshrc/.bashrc,
    // which can produce "ttyname error" noise and may hang in pipe environments.
    if use_login_shell && matches!(name, "bash" | "zsh" | "ksh") {
        "-lc".to_string()
    } else {
        "-c".to_string()
    }
}

fn process_sandbox_home() -> PathBuf {
    PathBuf::from("/tmp").join(format!(
        "rara-home-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ))
}

fn sandbox_home_dirs(sandbox_home: &Path) -> Vec<PathBuf> {
    vec![
        sandbox_home.to_path_buf(),
        sandbox_home.join(".config"),
        sandbox_home.join(".cache"),
        sandbox_home.join(".local"),
        sandbox_home.join(".local/state"),
        sandbox_home.join(".local/share"),
    ]
}

fn cleanup_stale_profiles(profile_dir: &Path) -> Result<()> {
    cleanup_profiles_older_than(profile_dir, PROFILE_CLEANUP_AGE)
}

fn cleanup_profiles_older_than(profile_dir: &Path, max_age: Duration) -> Result<()> {
    let now = SystemTime::now();
    for entry in fs::read_dir(profile_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path.is_file() && file_name.starts_with("sandbox-") && file_name.ends_with(".sb") {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let Ok(age) = now.duration_since(modified) else {
                continue;
            };
            if age >= max_age {
                let _ = fs::remove_file(&path);
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests;
