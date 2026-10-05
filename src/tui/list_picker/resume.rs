use super::*;
use crate::thread_store::ThreadSummary;
use crate::tui::display_sanitize::sanitize_display_line;
use crate::tui::text_wrap::{display_width, suffix_to_width, truncate_to_width};
use crate::tui::transcript_text::wrap_lines;

pub(super) fn render_picker(f: &mut Frame, app: &mut TuiApp, area: Rect) -> Option<(u16, u16)> {
    let block = popup_block().title(ListPickerKind::Resume.title());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let footer = wrap_lines(
        &[
            Line::from("Tab cwd/all  Ctrl+S sort"),
            Line::from("Ctrl+R retry  PgUp/PgDn page"),
            Line::from("Up/Down move  Enter resume"),
            Line::from("Esc clear/close"),
        ],
        inner.width,
    );
    let footer_height = u16::try_from(footer.len())
        .unwrap_or(u16::MAX)
        .min(inner.height.saturating_sub(4));
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(footer_height),
    ])
    .split(inner);
    let sort = if app.resume_sort_by_created {
        "created"
    } else {
        "updated"
    };
    let count = app.recent_threads.len();
    let more = if app.resume_query.has_more() { "+" } else { "" };
    let selected = if count == 0 {
        0
    } else {
        app.resume_picker_idx + 1
    };
    f.render_widget(
        Paragraph::new(format!(
            "{} | {sort} | {selected}/{count}{more}",
            app.resume_query.scope_label()
        )),
        chunks[0],
    );
    let status = app
        .resume_query
        .error
        .as_deref()
        .unwrap_or(if app.resume_query.loading {
            "Loading saved threads..."
        } else {
            "Search saved sessions and latest preview"
        });
    f.render_widget(Paragraph::new(sanitize_display_line(status)), chunks[1]);

    f.render_widget(Paragraph::new("Search: "), chunks[2]);
    let prefix_width = 8.min(chunks[2].width);
    let query_area = Rect {
        x: chunks[2].x.saturating_add(prefix_width),
        width: chunks[2].width.saturating_sub(prefix_width),
        ..chunks[2]
    };
    let query = &app.resume_search_query;
    let cursor_byte =
        crate::tui::state::char_offset_to_byte_index(query, app.resume_search_cursor_offset());
    let prefix = sanitize_display_line(&query[..cursor_byte]);
    let suffix = sanitize_display_line(&query[cursor_byte..]);
    let visible_prefix = suffix_to_width(&prefix, usize::from(query_area.width.saturating_sub(1)));
    let visible = format!("{visible_prefix}{suffix}");
    let cursor_column = display_width(visible_prefix) as u16;
    f.render_widget(
        Paragraph::new(truncate_to_width(&visible, query_area.width as usize)),
        query_area,
    );

    let items = render_items(app, app.resume_picker_idx);
    let item_height = items.iter().map(ListItem::height).max().unwrap_or(1).max(1);
    app.resume_query.page_items = (usize::from(chunks[3].height) / item_height).max(1);
    let mut state = list_picker_state(app.resume_picker_idx, count);
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(list_picker_highlight_style())
            .highlight_symbol("\u{203a} "),
        chunks[3],
        &mut state,
    );
    f.render_widget(Paragraph::new(footer), chunks[4]);
    (query_area.width > 0 && query_area.height > 0)
        .then_some((query_area.x.saturating_add(cursor_column), query_area.y))
}

pub(super) fn render_items(app: &TuiApp, selected: usize) -> Vec<ListItem<'static>> {
    if app.resume_query.loading && app.recent_threads.is_empty() {
        return vec![ListItem::new("Loading saved threads...")];
    }
    if app.recent_threads.is_empty()
        && let Some(error) = &app.resume_query.error
    {
        return vec![ListItem::new(
            crate::tui::display_sanitize::sanitize_display_text(error),
        )];
    }
    let summaries = resumable_threads(app);
    if summaries.is_empty() {
        return vec![ListItem::new("No threads available.")];
    }
    let now = current_unix_time_secs();
    summaries
        .iter()
        .enumerate()
        .map(|(idx, summary)| {
            ListItem::new(render_resume_summary_lines(idx, summary, now))
                .style(ListPickerKind::selected_style(idx, selected))
        })
        .collect()
}

pub(crate) fn selected_resumable_thread_id(app: &TuiApp) -> Option<String> {
    if app.recent_threads.is_empty() {
        return None;
    }
    resumable_threads(app)
        .get(app.resume_picker_idx)
        .map(|summary| summary.metadata.session_id.clone())
}

pub(crate) fn resumable_threads(app: &TuiApp) -> Vec<&ThreadSummary> {
    app.recent_threads
        .iter()
        .filter(|summary| summary.metadata.session_id != app.snapshot.session_id)
        .collect()
}

pub(super) fn render_resume_summary_lines(
    idx: usize,
    summary: &ThreadSummary,
    now: u64,
) -> Vec<Line<'static>> {
    let preview = normalized_resume_preview(summary);
    let metadata = &summary.metadata;
    let workspace = &metadata.cwd;
    let updated = format_resume_age(metadata.updated_at, now);
    let counts = format!(
        "hist={} trans={} compact={}",
        metadata.history_len, metadata.transcript_len, summary.compaction.compaction_count
    );
    let compaction = resume_compaction_detail(summary);

    let title = Line::from(vec![
        Span::raw(format!("[{}] ", idx + 1)),
        Span::styled(preview, Style::default().add_modifier(Modifier::BOLD)),
    ]);
    let details = Line::from(format!(
        "     {updated}  {}/{}  mode={} approval={}  {counts}",
        metadata.provider, metadata.model, metadata.agent_mode, metadata.bash_approval,
    ));

    let mut lines = vec![
        title,
        Line::from(format!("     cwd={workspace} branch={}", metadata.branch)),
        details,
    ];
    if let Some(compaction) = compaction {
        lines.push(Line::from(format!("     {compaction}")));
    }
    lines
}

fn normalized_resume_preview(summary: &ThreadSummary) -> String {
    let preview = summary
        .metadata
        .title
        .as_deref()
        .unwrap_or(&summary.preview)
        .replace('\n', " ");
    let preview = preview.trim();
    if preview.is_empty() {
        "(no transcript preview)".to_string()
    } else {
        preview.to_string()
    }
}

fn current_unix_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn format_resume_age(updated_at: i64, now: u64) -> String {
    if updated_at <= 0 {
        return "updated unknown".to_string();
    }
    let age = now.saturating_sub(updated_at as u64);
    let label = if age < 60 {
        "just now".to_string()
    } else if age < 3600 {
        format!("{}m ago", age / 60)
    } else if age < 86400 {
        format!("{}h ago", age / 3600)
    } else {
        format!("{}d ago", age / 86400)
    };
    format!("updated {label}")
}

fn resume_compaction_detail(summary: &ThreadSummary) -> Option<String> {
    let compaction = &summary.compaction;
    if compaction.compaction_count == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(version) = compaction.boundary_version {
        parts.push(format!("boundary=v{version}"));
    }
    if let Some(count) = compaction.recent_file_count {
        parts.push(format!("recent_files={count}"));
    }
    if let (Some(before), Some(after)) = (compaction.before_tokens, compaction.after_tokens) {
        parts.push(format!("tokens={before}->{after}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!("compact {}", parts.join(" ")))
    }
}
