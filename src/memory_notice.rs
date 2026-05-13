pub(crate) const MEMORY_NOTICE_PREFIX: &str = "Memory ·";

pub(crate) fn memory_notice(action: impl AsRef<str>) -> String {
    format!("{MEMORY_NOTICE_PREFIX} {}", action.as_ref())
}

pub(crate) fn count_label(label: &str, count: usize) -> String {
    if count == 1 {
        label.to_string()
    } else {
        format!("{label}s")
    }
}
