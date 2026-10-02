use std::rc::Rc;

use ratatui::{
    layout::Alignment,
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::CommittedTranscriptRenderCache;
use crate::tui::transcript_rows::TranscriptRows;

#[test]
fn unchanged_tail_retains_styled_and_text_rows() {
    let mut cache = CommittedTranscriptRenderCache::default();
    let lines = vec![
        Line::from("first"),
        Line::from("middle"),
        Line::from("last"),
    ];
    cache.update_active(lines.clone(), 20);
    let retained = cache.active.clone();
    let before = cache.work.get();
    for _ in 0..20 {
        cache.update_active(lines.clone(), 20);
        assert!(Rc::ptr_eq(&retained, &cache.active));
    }
    assert_eq!(cache.work.get(), before);
}

#[test]
fn every_middle_row_attribute_invalidates_the_tail() {
    let lines = vec![
        Line::from("first"),
        Line::from("middle"),
        Line::from("last"),
    ];
    let mutations = [
        Line::from("change"),
        Line::from("middle").style(Style::default().add_modifier(Modifier::ITALIC)),
        Line::from(Span::styled(
            "middle",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("middle").alignment(Alignment::Right),
    ];
    for middle in mutations {
        let mut cache = CommittedTranscriptRenderCache::default();
        cache.update_active(lines.clone(), 20);
        let retained = cache.active.clone();
        let mut changed = lines.clone();
        changed[1] = middle;
        cache.update_active(changed.clone(), 20);
        assert!(!Rc::ptr_eq(&retained, &cache.active));
        let rows = TranscriptRows::new(cache.history.clone(), cache.active.clone());
        assert_eq!(
            rows.iter().cloned().collect::<Vec<_>>(),
            crate::tui::transcript_text::wrap_lines(&changed, 20)
        );
    }
}

#[test]
fn unchanged_text_reflows_on_width_changes_and_clears_on_empty_tail() {
    let mut cache = CommittedTranscriptRenderCache::default();
    let lines = vec![Line::from("aaaa bbbb cccc")];
    cache.update_active(lines.clone(), 20);
    let retained = cache.active.clone();
    assert_eq!(retained.len(), 1);
    cache.update_active(lines, 4);
    assert!(!Rc::ptr_eq(&retained, &cache.active));
    assert_eq!(cache.active.len(), 3);
    cache.update_active(Vec::new(), 4);
    assert_eq!(cache.active.len(), 0);
    assert_eq!(retained.len(), 1);
}
