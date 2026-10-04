use std::collections::HashSet;

use super::*;
use crate::state_db::{PersistedCompactState, PersistedPromptRuntimeState, PersistedTurnEntry};

fn session(db: &StateDb, id: &str, cwd: &str) -> Result<()> {
    db.upsert_session(
        id,
        cwd,
        "main",
        "codex",
        "model",
        None,
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        1,
        1,
        &PersistedCompactState::default(),
    )
}

fn query(search: &str) -> ThreadListQuery<'_> {
    ThreadListQuery {
        search,
        cwd: None,
        exclude_session_id: None,
        sort: ThreadListSort::Updated,
        after: None,
        limit: 50,
    }
}

#[test]
fn search_reaches_old_rows_and_matches_literal_preview_and_metadata() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = StateDb::new_for_root_dir(dir.path().into())?;
    for i in 0..250 {
        session(&db, &format!("thread-{i:03}"), "/workspace")?;
    }
    db.persist_turn(
        "thread-000",
        0,
        &[PersistedTurnEntry {
            role: "You".into(),
            message: "MixedCase 100%_done \u{8def}\u{5f84}".into(),
        }],
    )?;
    db.conn.lock().map_err(|_| anyhow!("mutex"))?.execute(
        "UPDATE sessions SET updated_at = CASE WHEN id = 'thread-000' THEN 0 ELSE 10 END",
        [],
    )?;
    assert!(
        !db.list_recent_thread_records(200)?
            .iter()
            .any(|thread| thread.session_id == "thread-000")
    );
    for search in ["mixedcase", "100%_done", "\u{8def}\u{5f84}", "thread-000"] {
        let page = db.query_threads(query(search))?;
        assert_eq!(
            page.threads
                .iter()
                .map(|thread| thread.session_id.as_str())
                .collect::<Vec<_>>(),
            ["thread-000"]
        );
    }
    assert!(db.query_threads(query("100X_done"))?.threads.is_empty());
    assert_eq!(db.query_threads(query("MODEL"))?.threads.len(), 50);
    Ok(())
}

#[test]
fn full_cwd_and_current_session_filter_apply_before_limit() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = StateDb::new_for_root_dir(dir.path().into())?;
    session(&db, "a", "/a/app")?;
    session(&db, "b", "/b/app")?;
    session(&db, "current", "/a/app")?;
    let page = db.query_threads(ThreadListQuery {
        cwd: Some("/a/app"),
        exclude_session_id: Some("current"),
        ..query("")
    })?;
    assert_eq!(
        page.threads
            .iter()
            .map(|thread| thread.session_id.as_str())
            .collect::<Vec<_>>(),
        ["a"]
    );
    assert!(
        db.query_threads(ThreadListQuery {
            cwd: Some("/missing/app"),
            ..query("")
        })?
        .threads
        .is_empty()
    );
    Ok(())
}

#[test]
fn cursor_pages_cover_ties_exactly_once_in_both_orders() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = StateDb::new_for_root_dir(dir.path().into())?;
    for i in 0..213 {
        session(&db, &format!("thread-{i:03}"), "/workspace")?;
    }
    db.conn
        .lock()
        .map_err(|_| anyhow!("mutex"))?
        .execute("UPDATE sessions SET updated_at = 10, created_at = 20", [])?;
    for sort in [ThreadListSort::Updated, ThreadListSort::Created] {
        let mut cursor = None;
        let mut ids = Vec::new();
        for _ in 0..10 {
            let page = db.query_threads(ThreadListQuery {
                sort,
                after: cursor.as_ref(),
                ..query("")
            })?;
            assert!(page.threads.len() <= 50);
            ids.extend(page.threads.into_iter().map(|thread| thread.session_id));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert!(cursor.is_none());
        assert_eq!(ids.len(), 213);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 213);
        assert!(ids.windows(2).all(|pair| pair[0] > pair[1]));
    }
    db.conn.lock().map_err(|_| anyhow!("mutex"))?.execute(
        "UPDATE sessions SET updated_at=100, created_at=0 WHERE id='thread-000'",
        [],
    )?;
    assert_eq!(
        db.query_threads(query(""))?.threads[0].session_id,
        "thread-000"
    );
    assert_ne!(
        db.query_threads(ThreadListQuery {
            sort: ThreadListSort::Created,
            ..query("")
        })?
        .threads[0]
            .session_id,
        "thread-000"
    );
    Ok(())
}
