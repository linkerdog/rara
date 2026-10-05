use std::sync::Arc;

use super::*;

#[cfg(unix)]
async fn prepared(script: &str) -> PreparedEdit {
    PreparedEdit::prepare(
        EditorRequest {
            seed: Arc::from("original draft\n"),
            cwd: PathBuf::new(),
        },
        EditorCommand {
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), script.into(), "editor-fixture".into()],
        },
    )
    .await
    .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn editor_receives_exact_seed_and_quoted_file_arg_and_cleans_backup_files() {
    use std::os::unix::fs::PermissionsExt;
    let mut edit = prepared("test \"$(cat \"$1\")\" = 'original draft' || exit 3; printf backup > \"$1~\"; printf 'edited draft\\n' > \"$1\"").await;
    let directory = edit.directory.path().to_path_buf();
    assert_eq!(
        std::fs::metadata(&edit.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let unusual = directory.join("a ' $(touch sentinel) \u{754c}.md");
    std::fs::rename(&edit.path, &unusual).unwrap();
    edit.path = unusual;
    assert_eq!(edit.run().await.unwrap(), "edited draft\n");
    assert!(!directory.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn nonzero_missing_or_non_utf8_results_fail_and_cleanup_the_transaction() {
    for script in [
        "printf damaged > \"$1\"; exit 7",
        "rm \"$1\"",
        "printf '\\377' > \"$1\"",
    ] {
        let edit = prepared(script).await;
        let directory = edit.directory.path().to_path_buf();
        assert!(edit.run().await.is_err(), "{script}");
        assert!(!directory.exists());
    }
    let mut edit = prepared("exit 0").await;
    edit.command.program = edit
        .directory
        .path()
        .join("missing-editor")
        .to_string_lossy()
        .into_owned();
    let directory = edit.directory.path().to_path_buf();
    assert!(edit.run().await.is_err());
    assert!(!directory.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn successful_empty_edit_is_a_deliberate_empty_draft() {
    let edit = prepared(": > \"$1\"").await;
    assert_eq!(edit.run().await.unwrap(), "");
}
