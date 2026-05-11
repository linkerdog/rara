use std::env;
use std::path::PathBuf;
use std::time::Duration;

use tempfile::tempdir;

use super::{
    DEFAULT_SHELL, MACOS_SANDBOX_EXEC, SandboxBackend, SandboxManager, cleanup_profiles_older_than,
    cleanup_stale_profiles, command_search_install_roots, sandbox_profile_string_literal,
    sanitize_shell_program, shell_command_flag, shell_program,
};

fn manager(os: &str, backend: SandboxBackend) -> SandboxManager {
    SandboxManager {
        os: os.to_string(),
        profile_dir: PathBuf::from("/tmp/rara-test-sandbox"),
        sandbox_home: PathBuf::from("/tmp/rara-test-sandbox-home"),
        backend,
        command_install_roots: command_search_install_roots(std::env::var_os("PATH").as_deref()),
    }
}

fn set_env_var<K, V>(key: K, value: V)
where
    K: AsRef<std::ffi::OsStr>,
    V: AsRef<std::ffi::OsStr>,
{
    // These tests restore PATH immediately after the scoped assertion.
    unsafe { std::env::set_var(key, value) };
}

fn remove_env_var<K>(key: K)
where
    K: AsRef<std::ffi::OsStr>,
{
    // These tests restore PATH immediately after the scoped assertion.
    unsafe { std::env::remove_var(key) };
}

#[test]
fn wrap_command_fails_closed_on_unsupported_platform() {
    let manager = manager(
        "freebsd",
        SandboxBackend::Unsupported {
            platform: "freebsd".to_string(),
        },
    );

    let err = manager
        .wrap_shell_command("echo test", "/tmp", false)
        .expect_err("unsupported platforms should not fall back to unsandboxed execution");

    assert!(
        err.to_string()
            .contains("sandboxed command execution is unsupported on platform freebsd")
    );
}

#[test]
fn detect_fails_closed_when_macos_sandbox_exec_is_unavailable() {
    let backend = SandboxBackend::detect("macos");

    if PathBuf::from(MACOS_SANDBOX_EXEC).is_file() {
        assert_eq!(backend, SandboxBackend::MacosSeatbelt);
    } else {
        assert!(matches!(
            backend,
            SandboxBackend::Unsupported { platform } if platform.contains("sandbox unavailable")
        ));
    }
}

#[test]
fn detect_fails_closed_when_linux_bwrap_is_unavailable() {
    let original_path = std::env::var_os("PATH");
    set_env_var("PATH", "");
    let backend = SandboxBackend::detect("linux");
    if let Some(path) = original_path {
        set_env_var("PATH", path);
    } else {
        remove_env_var("PATH");
    }

    assert!(matches!(
        backend,
        SandboxBackend::Unsupported { platform } if platform.contains("install bubblewrap")
    ));
}

#[test]
fn wrap_command_creates_unique_cleanup_profile_on_macos() {
    let tempdir = tempdir().expect("tempdir");
    let manager = SandboxManager {
        os: "macos".to_string(),
        profile_dir: tempdir.path().to_path_buf(),
        sandbox_home: tempdir.path().join("home"),
        backend: SandboxBackend::MacosSeatbelt,
        command_install_roots: command_search_install_roots(std::env::var_os("PATH").as_deref()),
    };

    let wrapped = manager
        .wrap_shell_command("echo test", "/tmp", false)
        .expect("macos sandbox wrapper");

    assert_eq!(wrapped.program, MACOS_SANDBOX_EXEC);
    assert!(wrapped.sandboxed);
    assert_eq!(wrapped.sandbox_backend, "macos-seatbelt");
    assert_eq!(
        wrapped.sandbox_home.as_deref(),
        Some(manager.sandbox_home.as_path())
    );

    let cleanup_path = wrapped
        .cleanup_path
        .expect("macos wrapper should return cleanup path");

    assert!(cleanup_path.exists(), "profile should be created on disk");
    assert!(
        wrapped
            .args
            .iter()
            .any(|arg| arg == &cleanup_path.display().to_string()),
        "wrapped command should reference the generated profile path"
    );

    let profile = std::fs::read_to_string(&cleanup_path).expect("profile contents");
    assert!(
        !profile.contains("home-relative-path"),
        "profile should avoid unsupported home-relative-path forms"
    );
    assert!(
        profile.contains("(deny file-read* (subpath "),
        "profile should deny sensitive home subpaths using explicit paths"
    );
}

#[test]
fn wrap_command_can_be_explicitly_direct() {
    let manager = manager("macos", SandboxBackend::Direct);

    let wrapped = manager
        .wrap_shell_command("find . -maxdepth 1", "/workspace/project", false)
        .expect("direct fallback wrapper");

    assert_eq!(wrapped.program, shell_program());
    assert!(matches!(
        wrapped.args.as_slice(),
        [flag, command] if (flag == "-c" || flag == "-lc") && command == "find . -maxdepth 1"
    ));
    assert!(wrapped.cleanup_path.is_none());
    assert!(
        !wrapped.sandboxed,
        "direct execution should not apply sandbox env or profiles"
    );
    assert_eq!(wrapped.sandbox_backend, "direct");
    assert!(wrapped.sandbox_home.is_none());
}

#[test]
fn wrap_pty_shell_command_uses_direct_backend_on_macos() {
    let manager = manager("macos", SandboxBackend::MacosSeatbelt);

    let wrapped = manager
        .wrap_pty_shell_command("read line", "/workspace/project", false)
        .expect("pty shell wrapper");

    assert!(!wrapped.sandboxed);
    assert_eq!(wrapped.sandbox_backend, "direct");
}

#[test]
fn shell_command_flag_obeys_use_login_shell_param() {
    // Default: non-login (-c) to avoid sourcing .zshrc/.bashrc.
    assert_eq!(shell_command_flag("/bin/zsh", false), "-c");
    assert_eq!(shell_command_flag("/usr/bin/bash", false), "-c");
    assert_eq!(shell_command_flag("sh", false), "-c");
    // When use_login_shell is true, bash/zsh get -lc; sh stays -c.
    assert_eq!(shell_command_flag("/bin/zsh", true), "-lc");
    assert_eq!(shell_command_flag("/usr/bin/bash", true), "-lc");
    assert_eq!(shell_command_flag("sh", true), "-c");
}

#[test]
fn sanitize_shell_program_rejects_args_and_unmounted_shells() {
    assert_eq!(
        sanitize_shell_program("/bin/zsh"),
        Some("/bin/zsh".to_string())
    );
    assert_eq!(
        sanitize_shell_program("/bin/zsh -i"),
        Some("/bin/zsh".to_string())
    );
    assert_eq!(sanitize_shell_program("/opt/homebrew/bin/fish"), None);
    assert_eq!(sanitize_shell_program("zsh"), None);
    assert_eq!(DEFAULT_SHELL, "/bin/sh");
}

#[test]
fn new_keeps_recent_macos_profiles() {
    let tempdir = tempdir().expect("tempdir");
    let rara_dir = tempdir.path().join(".rara");
    let profile_dir = rara_dir.join("sandbox");
    std::fs::create_dir_all(&profile_dir).expect("profile dir");
    let recent_profile = profile_dir.join("sandbox-recent.sb");
    std::fs::write(&recent_profile, "(version 1)").expect("recent profile");

    let manager = SandboxManager::new_for_rara_dir(rara_dir.clone()).expect("sandbox manager");

    assert!(
        manager.profile_dir == rara_dir.join("sandbox"),
        "sandbox manager should point at the configured sandbox dir"
    );
    assert!(
        recent_profile.exists(),
        "recent sandbox profiles should not be removed on startup"
    );
}

#[test]
fn cleanup_removes_profiles_when_they_are_older_than_the_threshold() {
    let tempdir = tempdir().expect("tempdir");
    let stale_profile = tempdir.path().join("sandbox-stale.sb");
    let unrelated_file = tempdir.path().join("notes.txt");
    std::fs::write(&stale_profile, "(version 1)").expect("stale profile");
    std::fs::write(&unrelated_file, "keep").expect("unrelated file");

    cleanup_profiles_older_than(tempdir.path(), Duration::ZERO).expect("cleanup");

    assert!(
        !stale_profile.exists(),
        "matching profiles should be removed when past the cleanup threshold"
    );
    assert!(
        unrelated_file.exists(),
        "non-profile files must not be removed by sandbox cleanup"
    );
}

#[test]
fn cleanup_keeps_recent_profiles_with_default_threshold() {
    let tempdir = tempdir().expect("tempdir");
    let recent_profile = tempdir.path().join("sandbox-recent.sb");
    std::fs::write(&recent_profile, "(version 1)").expect("recent profile");

    cleanup_stale_profiles(tempdir.path()).expect("cleanup");

    assert!(
        recent_profile.exists(),
        "startup cleanup should not remove profiles that may belong to active instances"
    );
}

#[test]
fn process_sandbox_home_uses_tmp_root_and_unique_names() {
    let first = super::process_sandbox_home();
    let second = super::process_sandbox_home();

    assert!(first.starts_with("/tmp"));
    assert!(second.starts_with("/tmp"));
    assert_ne!(
        first, second,
        "sandbox home paths must not collide between manager instances"
    );
}

#[test]
fn wrap_shell_command_uses_minimal_linux_bind_set() {
    let manager = manager("linux", SandboxBackend::LinuxBubblewrap);

    let wrapped = manager
        .wrap_shell_command("echo test", "/workspace/project", false)
        .expect("linux sandbox wrapper");

    assert_eq!(wrapped.program, "bwrap");
    assert!(
        !wrapped.args.windows(3).any(|window| window
            == [
                String::from("--ro-bind"),
                String::from("/"),
                String::from("/")
            ]),
        "linux sandbox should no longer bind the entire filesystem read-only"
    );
    assert!(
        wrapped
            .args
            .windows(2)
            .any(|window| window == [String::from("--tmpfs"), String::from("/")]),
        "linux sandbox should start from an empty root filesystem"
    );
    assert!(
        wrapped
            .args
            .windows(2)
            .any(|window| window == [String::from("--tmpfs"), String::from("/tmp")]),
        "linux sandbox should provide an isolated writable /tmp"
    );
    assert!(
        wrapped
            .args
            .windows(2)
            .any(|window| window == [String::from("--bind"), String::from("/workspace/project")]),
        "linux sandbox should bind the workspace path back in"
    );
    assert!(
        wrapped.args.contains(&"--unshare-net".to_string()),
        "linux sandbox should isolate networking when allow_net is false"
    );
    assert_eq!(wrapped.sandbox_backend, "linux-bubblewrap");
    assert_eq!(
        wrapped.sandbox_home.as_deref(),
        Some(manager.sandbox_home.as_path())
    );
}

#[test]
fn linux_sandbox_keeps_network_when_allowed() {
    let manager = manager("linux", SandboxBackend::LinuxBubblewrap);

    let wrapped = manager
        .wrap_shell_command("curl https://example.com", "/workspace/project", true)
        .expect("linux sandbox wrapper");

    assert!(
        !wrapped.args.contains(&"--unshare-net".to_string()),
        "linux sandbox should not isolate networking when allow_net is true"
    );
    assert!(wrapped.network_access);
}

#[test]
fn linux_sandbox_creates_home_dirs_inside_bubblewrap() {
    let manager = manager("linux", SandboxBackend::LinuxBubblewrap);

    let wrapped = manager
        .wrap_shell_command("echo test", "/workspace/project", false)
        .expect("linux sandbox wrapper");

    for dir in [
        manager.sandbox_home.clone(),
        manager.sandbox_home.join(".config"),
        manager.sandbox_home.join(".cache"),
        manager.sandbox_home.join(".local/state"),
        manager.sandbox_home.join(".local/share"),
    ] {
        assert!(
            wrapped
                .args
                .windows(2)
                .any(|window| { window == [String::from("--dir"), dir.display().to_string()] }),
            "linux sandbox should create {} inside the tmpfs root",
            dir.display()
        );
    }
}

#[test]
fn linux_sandbox_does_not_bind_the_entire_home_directory() {
    let manager = manager("linux", SandboxBackend::LinuxBubblewrap);

    let wrapped = manager
        .wrap_shell_command("echo test", "/home/tester/work/project", false)
        .expect("linux sandbox wrapper");

    assert_eq!(wrapped.program, "bwrap");
    assert!(
        !wrapped.args.windows(3).any(|window| {
            window
                == [
                    String::from("--bind"),
                    String::from("/home/tester"),
                    String::from("/home/tester"),
                ]
                || window
                    == [
                        String::from("--ro-bind"),
                        String::from("/home/tester"),
                        String::from("/home/tester"),
                    ]
        }),
        "linux sandbox should not mount the entire home directory back in"
    );
    assert!(
        wrapped.args.windows(3).any(|window| {
            window
                == [
                    String::from("--bind"),
                    String::from("/home/tester/work/project"),
                    String::from("/home/tester/work/project"),
                ]
        }),
        "linux sandbox should still mount the workspace path itself"
    );
}

#[test]
fn linux_sandbox_binds_command_search_path_dirs() {
    let tempdir = tempdir().expect("tempdir");
    let custom_bin = tempdir.path().join("custom-bin");
    std::fs::create_dir_all(&custom_bin).expect("custom bin dir");
    let custom_bin = std::fs::canonicalize(&custom_bin).expect("canonical custom bin");
    let original_path = std::env::var_os("PATH");
    let test_path =
        env::join_paths([PathBuf::from("."), custom_bin.clone()]).expect("build test PATH");
    set_env_var("PATH", test_path);

    let manager = manager("linux", SandboxBackend::LinuxBubblewrap);
    let wrapped = manager
        .wrap_shell_command("custom-tool --version", "/workspace/project", false)
        .expect("linux sandbox wrapper");

    if let Some(path) = original_path {
        set_env_var("PATH", path);
    } else {
        remove_env_var("PATH");
    }

    assert!(
        wrapped.args.windows(3).any(|window| {
            window
                == [
                    String::from("--ro-bind"),
                    custom_bin.display().to_string(),
                    custom_bin.display().to_string(),
                ]
        }),
        "linux sandbox should bind PATH command directories that are outside standard runtime roots"
    );
    assert!(
        !wrapped.args.windows(3).any(|window| {
            window
                == [
                    String::from("--ro-bind"),
                    String::from("."),
                    String::from("."),
                ]
        }),
        "linux sandbox should not bind relative PATH entries"
    );
}

#[test]
fn linux_sandbox_binds_command_install_roots_for_bin_dirs() {
    let tempdir = tempdir().expect("tempdir");
    let tool_root = tempdir.path().join("toolchain");
    let tool_bin = tool_root.join("bin");
    std::fs::create_dir_all(&tool_bin).expect("tool bin dir");
    let tool_root = std::fs::canonicalize(&tool_root).expect("canonical tool root");
    let tool_bin = tool_root.join("bin");
    let original_path = std::env::var_os("PATH");
    set_env_var("PATH", &tool_bin);

    let manager = manager("linux", SandboxBackend::LinuxBubblewrap);
    let wrapped = manager
        .wrap_shell_command("custom-tool --version", "/workspace/project", false)
        .expect("linux sandbox wrapper");

    if let Some(path) = original_path {
        set_env_var("PATH", path);
    } else {
        remove_env_var("PATH");
    }

    assert!(
        wrapped.args.windows(3).any(|window| {
            window
                == [
                    String::from("--ro-bind"),
                    tool_root.display().to_string(),
                    tool_root.display().to_string(),
                ]
        }),
        "linux sandbox should bind the PATH install root so symlinked launchers and adjacent libraries remain readable"
    );
}

#[test]
fn macos_profile_allows_command_install_roots() {
    let tempdir = tempdir().expect("tempdir");
    let tool_root = tempdir.path().join("toolchain");
    let tool_bin = tool_root.join("bin");
    std::fs::create_dir_all(&tool_bin).expect("tool bin dir");
    let tool_root = std::fs::canonicalize(&tool_root).expect("canonical tool root");
    let original_path = std::env::var_os("PATH");
    set_env_var("PATH", tool_root.join("bin"));
    let manager = SandboxManager {
        os: "macos".to_string(),
        profile_dir: tempdir.path().to_path_buf(),
        sandbox_home: tempdir.path().join("home"),
        backend: SandboxBackend::MacosSeatbelt,
        command_install_roots: command_search_install_roots(Some(
            tool_root.join("bin").as_os_str(),
        )),
    };

    let profile_path = manager.create_profile(false).expect("macos profile");

    if let Some(path) = original_path {
        set_env_var("PATH", path);
    } else {
        remove_env_var("PATH");
    }

    let profile = std::fs::read_to_string(profile_path).expect("profile contents");
    let expected = tool_root.display().to_string();
    assert!(
        profile.contains(&format!("(allow file-read* (subpath \"{expected}\"))")),
        "macos profile should allow reading PATH install roots"
    );
    assert!(
        profile.contains(&format!(
            "(allow file-map-executable (subpath \"{expected}\"))"
        )),
        "macos profile should allow mapping executables from PATH install roots"
    );
}

#[test]
fn home_bin_path_stays_narrow() {
    let tempdir = tempdir().expect("tempdir");
    let home = tempdir.path().join("home");
    let home_bin = home.join("bin");
    std::fs::create_dir_all(&home_bin).expect("home bin dir");
    let home = std::fs::canonicalize(&home).expect("canonical home");
    let home_bin = home.join("bin");
    let original_home = std::env::var_os("HOME");
    set_env_var("HOME", &home);

    let roots = command_search_install_roots(Some(home_bin.as_os_str()));

    if let Some(home) = original_home {
        set_env_var("HOME", home);
    } else {
        remove_env_var("HOME");
    }

    assert_eq!(roots, vec![home_bin]);
}

#[test]
fn command_search_install_roots_rejects_broad_path_entries() {
    let tempdir = tempdir().expect("tempdir");
    let home = tempdir.path().join("home");
    let tool_root = tempdir.path().join("toolchain");
    let tool_bin = tool_root.join("bin");
    std::fs::create_dir_all(&home).expect("home dir");
    std::fs::create_dir_all(&tool_bin).expect("tool bin dir");
    let home = std::fs::canonicalize(&home).expect("canonical home");
    let tool_root = std::fs::canonicalize(&tool_root).expect("canonical tool root");
    let tool_bin = tool_root.join("bin");
    let original_home = std::env::var_os("HOME");
    set_env_var("HOME", &home);

    let path = env::join_paths([PathBuf::from("/"), home.clone(), tool_bin.clone()])
        .expect("build test PATH");
    let roots = command_search_install_roots(Some(path.as_os_str()));

    if let Some(home) = original_home {
        set_env_var("HOME", home);
    } else {
        remove_env_var("HOME");
    }

    assert_eq!(roots, vec![tool_root]);
}

#[test]
fn sandbox_profile_string_literal_escapes_control_characters() {
    let escaped = sandbox_profile_string_literal(PathBuf::from("/tmp/a\nb\tc").as_path());

    assert_eq!(escaped, "/tmp/a\\nb\\tc");
}

#[test]
fn wrap_exec_command_fails_closed_on_unsupported_platform() {
    let manager = manager(
        "freebsd",
        SandboxBackend::Unsupported {
            platform: "freebsd".to_string(),
        },
    );

    let err = manager
        .wrap_exec_command("cargo", &["check".to_string()], "/tmp", false)
        .expect_err("unsupported platforms should not fall back to unsandboxed execution");

    assert!(
        err.to_string()
            .contains("sandboxed command execution is unsupported on platform freebsd")
    );
}

#[test]
fn wrap_exec_command_unsandboxed_exec_runs_directly() {
    let manager = manager("macos", SandboxBackend::Direct);

    let wrapped = manager
        .wrap_unsandboxed_exec_command("cargo", &["check".to_string(), "--workspace".to_string()]);

    assert_eq!(wrapped.program, "cargo");
    assert_eq!(wrapped.args, vec!["check", "--workspace"]);
    assert!(!wrapped.sandboxed);
    assert_eq!(wrapped.sandbox_backend, "direct");
}

#[test]
fn wrap_exec_command_direct_fallback() {
    let manager = manager("macos", SandboxBackend::Direct);

    let wrapped = manager
        .wrap_exec_command("cargo", &["test".to_string()], "/workspace", false)
        .expect("direct exec");

    assert_eq!(wrapped.program, "cargo");
    assert_eq!(wrapped.args, vec!["test"]);
    assert!(!wrapped.sandboxed);
    assert_eq!(wrapped.sandbox_backend, "direct");
    assert!(wrapped.network_access);
    assert!(wrapped.cleanup_path.is_none());
}

#[test]
fn wrap_exec_command_macos_seatbelt() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let manager = SandboxManager {
        os: "macos".to_string(),
        profile_dir: tempdir.path().to_path_buf(),
        sandbox_home: tempdir.path().join("home"),
        backend: SandboxBackend::MacosSeatbelt,
        command_install_roots: command_search_install_roots(std::env::var_os("PATH").as_deref()),
    };

    let wrapped = manager
        .wrap_exec_command(
            "cargo",
            &["check".to_string(), "--workspace".to_string()],
            "/tmp",
            false,
        )
        .expect("macos exec wrapper");

    assert_eq!(wrapped.program, MACOS_SANDBOX_EXEC);
    assert!(wrapped.sandboxed);
    assert_eq!(wrapped.sandbox_backend, "macos-seatbelt");

    let cleanup_path = wrapped
        .cleanup_path
        .expect("macos wrapper should return cleanup path");
    assert!(cleanup_path.exists());

    assert!(
        wrapped.args.contains(&"cargo".to_string()),
        "wrapped args should contain the program name"
    );
    assert!(
        wrapped.args.iter().any(|arg| arg == "check"),
        "wrapped args should contain program args"
    );
    assert!(
        wrapped.args.iter().any(|arg| arg == "--workspace"),
        "wrapped args should contain all program args"
    );
}

#[test]
fn wrap_exec_command_linux_bubblewrap() {
    let manager = manager("linux", SandboxBackend::LinuxBubblewrap);

    let wrapped = manager
        .wrap_exec_command("cargo", &["check".to_string()], "/workspace/project", false)
        .expect("linux exec wrapper");

    assert_eq!(wrapped.program, "bwrap");
    assert!(wrapped.sandboxed);
    assert_eq!(wrapped.sandbox_backend, "linux-bubblewrap");

    let dash_dash_pos = wrapped.args.iter().position(|a| a == "--");
    assert!(dash_dash_pos.is_some(), "-- separator should be present");
    let after_sep = &wrapped.args[dash_dash_pos.unwrap() + 1..];
    assert_eq!(after_sep[0], "cargo");
    assert_eq!(after_sep[1], "check");
    assert_eq!(after_sep.len(), 2);
}
