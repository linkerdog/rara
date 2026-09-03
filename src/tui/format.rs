pub(crate) fn cache_hit_rate_label(hit_tokens: u32, miss_tokens: u32) -> Option<String> {
    let total = hit_tokens.saturating_add(miss_tokens);
    (total > 0).then(|| format!("{:.1}%", hit_tokens as f64 * 100.0 / total as f64))
}

pub(crate) fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}
