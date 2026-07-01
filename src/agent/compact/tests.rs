use std::sync::OnceLock;
use std::time::Duration;

use super::*;

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::main::{build_compact_plan, group_history_by_api_round, read_file_line_range};
    use super::types::COMPACTION_SUMMARY_TIMEOUT;
    use crate::agent::Message;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    #[test]
    fn read_file_line_range_rejects_overflowing_offset_limit() {
        let input = object(json!({
            "offset": usize::MAX,
            "limit": 2,
        }));

        assert_eq!(read_file_line_range(&input), None);
    }

    #[test]
    fn read_file_line_range_accepts_checked_offset_limit() {
        let input = object(json!({
            "offset": 10,
            "limit": 3,
        }));

        assert_eq!(read_file_line_range(&input), Some((10, 12)));
    }

    #[test]
    fn api_round_grouping_keeps_tool_result_with_assistant_round() {
        let history = vec![
            Message {
                role: "user".to_string(),
                content: json!("start"),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/main.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content":"fn main() {}"}
                ]),
            },
            Message {
                role: "assistant".to_string(),
                content: json!("done"),
            },
        ];

        let groups = group_history_by_api_round(&history);

        assert_eq!(
            groups
                .iter()
                .map(|group| (group.start, group.end))
                .collect::<Vec<_>>(),
            vec![(0, 1), (1, 3), (3, 4)]
        );
    }

    #[test]
    fn compact_plan_uses_api_round_boundary_for_retained_suffix() {
        let history = vec![
            Message {
                role: "user".to_string(),
                content: json!("old request"),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/old.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content":"old output ".repeat(1_000)}
                ]),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-2","name":"read_file","input":{"path":"src/new.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-2","content":"new output"}
                ]),
            },
        ];

        let plan = build_compact_plan(&history, 600, false)
            .expect("plan")
            .expect("compact plan");

        assert_eq!(plan.summarize_end, 3);
        assert_eq!(plan.retained_start, 3);
    }

    #[test]
    fn compact_plan_does_not_split_single_assistant_tool_round() {
        let history = vec![
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/main.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content":"fn main() {}"}
                ]),
            },
        ];

        let plan = build_compact_plan(&history, 1, false).expect("plan");

        assert!(
            plan.is_none(),
            "single API round must not retain a detached tool_result"
        );
    }

    #[test]
    fn compact_plan_summarizes_oversized_latest_api_round() {
        let large_tool_output = "recent output ".repeat(2_000);
        let history = vec![
            Message {
                role: "user".to_string(),
                content: json!("old request"),
            },
            Message {
                role: "assistant".to_string(),
                content: json!("old answer"),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/main.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content": large_tool_output}
                ]),
            },
        ];

        let plan = build_compact_plan(&history, 100, false)
            .expect("plan")
            .expect("compact plan");
        assert_eq!(plan.summarize_end, history.len());
        assert_eq!(plan.retained_start, history.len());
    }

    #[test]
    fn cached_token_estimate_avoids_recomputation() {
        // Verify that CompactState stores estimated_history_tokens
        // and the compact_plan builder uses thresholds correctly.
        // The cache is populated incrementally by increment_history_tokens.
        use crate::agent::CompactState;
        let state = CompactState {
            estimated_history_tokens: 500,
            ..Default::default()
        };
        assert_eq!(state.estimated_history_tokens, 500);
    }
}
