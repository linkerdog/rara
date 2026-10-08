use std::fs;

use tempfile::tempdir;

use super::AgentTraceRecorder;
use crate::{
    AGENT_TRACE_SCHEMA_VERSION, AgentTraceEvent, CacheUsage, TraceManifest, TraceRecord,
    TurnStarted,
};

#[test]
fn recorder_writes_a_manifest_and_ordered_content_free_events()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempdir()?;
    let recorder = AgentTraceRecorder::new(root.path(), "session/one")?;
    let location = recorder.location().ok_or("trace location missing")?;
    let second_recorder = AgentTraceRecorder::new(root.path(), "session/one")?;
    let second_location = second_recorder
        .location()
        .ok_or("second trace location missing")?;

    recorder.record(
        Some("turn-1"),
        AgentTraceEvent::TurnStarted(TurnStarted {
            history_len: 3,
            memory_facilities_enabled: true,
        }),
    )?;
    recorder.record(
        Some("turn-1"),
        AgentTraceEvent::TurnStarted(TurnStarted {
            history_len: 4,
            memory_facilities_enabled: true,
        }),
    )?;

    let manifest: TraceManifest =
        serde_json::from_str(&fs::read_to_string(&location.manifest_path)?)?;
    let events = fs::read_to_string(&location.events_path)?
        .lines()
        .map(serde_json::from_str::<TraceRecord>)
        .collect::<Result<Vec<_>, _>>()?;

    assert_eq!(manifest.schema_version, AGENT_TRACE_SCHEMA_VERSION);
    assert_eq!(manifest.session_id, "session/one");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].sequence, 0);
    assert_eq!(events[1].sequence, 1);
    assert_eq!(events[0].turn_id.as_deref(), Some("turn-1"));
    assert!(!location.directory.to_string_lossy().contains("session/one"));
    assert_ne!(location.directory, second_location.directory);
    Ok(())
}

#[test]
fn cache_usage_distinguishes_empty_and_verified_zero_hit_receipts() {
    assert_eq!(CacheUsage::default().hit_rate_basis_points(), None);
    assert_eq!(
        CacheUsage {
            hit_tokens: 0,
            miss_tokens: 32,
        }
        .hit_rate_basis_points(),
        Some(0)
    );
}
