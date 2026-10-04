//! Shared provider-neutral pairing primitives used by transcript and provider adapters.
pub(crate) use rara_agent::{
    has_tool_result_block, keep_or_drop_tool_results, synthetic_tool_result_blocks,
    tool_use_ids_in_blocks,
};
