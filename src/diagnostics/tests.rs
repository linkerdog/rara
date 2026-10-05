use super::*;

#[test]
fn pending_records_are_bounded_and_overflow_is_visible() {
    let mut queue = PendingDiagnostics::default();
    for index in 0..MAX_PENDING + 3 {
        queue.push(Diagnostic::new(Level::Warn, format!("warning {index}")));
    }
    let records = queue.drain();
    assert_eq!(records.len(), MAX_PENDING + 1);
    assert_eq!(records[0].message, "warning 3");
    assert_eq!(
        records.last().unwrap().message,
        "Diagnostic queue overflow: 3 earlier messages were omitted."
    );
    assert!(queue.drain().is_empty());
}

#[test]
fn consecutive_duplicates_stay_coalesced_across_drains() {
    let mut queue = PendingDiagnostics::default();
    let failure = Diagnostic::new(Level::Warn, "transcript write failed".into());
    queue.push(failure.clone());
    assert_eq!(queue.drain(), vec![failure.clone()]);
    queue.push(failure.clone());
    assert!(queue.drain().is_empty());
    queue.push(Diagnostic::new(Level::Error, failure.message.clone()));
    queue.push(failure);
    assert_eq!(queue.drain().len(), 2);
}

#[test]
fn diagnostic_text_is_redacted_before_truncation_and_queueing() {
    let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
    let diagnostic = Diagnostic::new(
        Level::Error,
        format!("api_key={secret} {}", "界".repeat(MAX_MESSAGE_BYTES)),
    );
    assert_eq!(diagnostic.level, Level::Error);
    assert!(!diagnostic.message.contains(secret));
    assert!(diagnostic.message.len() <= MAX_MESSAGE_BYTES);
    assert!(diagnostic.message.ends_with(" [truncated]"));
}
