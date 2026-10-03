use std::sync::Arc;

use rara_state::state_db::StateDb;
use rara_tools::tool::Tool;
use serde_json::json;

use super::super::{RuntimeBootstrapOptions, initialize_rara_context_for_workspace_with_options};
use crate::config::RaraConfig;
use crate::runtime_goals::RalphGoal;
use crate::tools::goal::CreateGoalTool;

#[tokio::test]
async fn corrupt_goal_disables_only_goal_persistence_during_bootstrap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let home = dir.path().join("state");
    let data = rara_config::workspace_data_dir_for_home(&workspace, &home).expect("data root");
    let db = StateDb::new_for_root_dir(data).expect("state db");
    let mut corrupt =
        serde_json::to_value(RalphGoal::new("corrupt goal".into(), None)).expect("serialize goal");
    corrupt["status"] = json!("Unknown");
    db.save_goal("corrupt-runtime-thread", &corrupt)
        .expect("corrupt row");
    let stored = db
        .try_load_goal("corrupt-runtime-thread")
        .expect("seeded row");
    let config = RaraConfig {
        provider: "mock".into(),
        ..Default::default()
    };
    let bootstrap = initialize_rara_context_for_workspace_with_options(
        &config,
        Some(&workspace),
        None,
        RuntimeBootstrapOptions::default()
            .with_rara_home(Some(home))
            .with_session_id(Some("corrupt-runtime-thread".into()))
            .with_extension_discovery(false),
    )
    .await
    .expect("optional corrupt goal must not abort bootstrap");
    assert!(
        bootstrap
            .warnings
            .iter()
            .any(|warning| warning.contains("Goal persistence unavailable"))
    );
    assert!(bootstrap.goal_handle.snapshot().is_none());
    let tool = CreateGoalTool {
        store: bootstrap.goal_handle.clone(),
    };
    let error = tool
        .call(json!({"objective": "must not become memory-only"}))
        .await
        .expect_err("unavailable persistence rejects creation");
    assert!(error.to_string().contains("goal persistence unavailable"));
    assert_eq!(
        db.try_load_goal("corrupt-runtime-thread")
            .expect("stored row"),
        stored
    );
    assert_eq!(
        bootstrap.into_agent().await.session_id,
        "corrupt-runtime-thread"
    );
}

#[tokio::test]
async fn bootstrap_goal_binding_matches_explicit_and_generated_agent_ids() {
    for session_id in [Some("explicit-runtime-thread".to_string()), None] {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let config = RaraConfig {
            provider: "mock".into(),
            ..Default::default()
        };
        let bootstrap = initialize_rara_context_for_workspace_with_options(
            &config,
            Some(&workspace),
            None,
            RuntimeBootstrapOptions::default()
                .with_rara_home(Some(dir.path().join("state")))
                .with_session_id(session_id.clone())
                .with_extension_discovery(false),
        )
        .await
        .expect("bootstrap");
        let db = Arc::new(
            StateDb::new_for_root_dir(bootstrap.workspace.rara_dir.clone()).expect("state db"),
        );
        let expected = RalphGoal::new("bind the assembled agent".into(), None);
        bootstrap
            .goal_handle
            .replace(Some(expected.clone()))
            .expect("persist goal");
        let agent = bootstrap.into_agent().await;
        if let Some(session_id) = session_id {
            assert_eq!(agent.session_id, session_id);
        }
        assert_eq!(
            crate::runtime_goals::GoalStore::default()
                .restore_for_thread(&agent.session_id, db)
                .expect("restore actual agent binding"),
            Some(expected)
        );
    }
}
