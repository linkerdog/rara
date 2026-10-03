use super::progress::{self, ProgressRole};
use crate::tui::state::TranscriptEntry;

pub(super) enum OrderedActiveSegment<'a> {
    Exploration(Vec<String>),
    Progress(ProgressRole, Vec<String>),
    Agent(&'a str),
}

pub(super) fn ordered_exploration_agent_segments<'a>(
    current_turn: &[&'a TranscriptEntry],
    force_ordered_agent: bool,
) -> Option<Vec<OrderedActiveSegment<'a>>> {
    let mut segments = Vec::new();
    let mut exploration_items = Vec::new();
    let mut saw_interleaving = false;

    let flush_exploration = |segments: &mut Vec<OrderedActiveSegment<'a>>,
                             items: &mut Vec<String>| {
        if !items.is_empty() {
            segments.push(OrderedActiveSegment::Exploration(std::mem::take(items)));
        }
    };

    for entry in current_turn {
        match entry.role.as_str() {
            "Tool" => {
                if let Some(action) = super::super::exploration_action_label(&entry.message) {
                    exploration_items.push(action);
                }
            }
            "Exploring" => {
                for item in entry
                    .message
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|line| {
                        line.trim_start_matches("└")
                            .trim_start_matches("•")
                            .trim()
                            .to_string()
                    })
                    .filter(|line| !line.is_empty())
                {
                    exploration_items.push(item);
                }
            }
            role if let Some(progress_role) = ProgressRole::from_entry_role(role) => {
                let messages =
                    progress::progress_entry_message_lines(progress_role, &entry.message);
                if messages.is_empty() {
                    continue;
                }
                let continues_progress_group = matches!(
                    segments.last(),
                    Some(OrderedActiveSegment::Progress(last_role, _))
                        if *last_role == progress_role
                );
                if !exploration_items.is_empty()
                    || (!continues_progress_group && !segments.is_empty())
                {
                    saw_interleaving = true;
                }
                flush_exploration(&mut segments, &mut exploration_items);
                if let Some(OrderedActiveSegment::Progress(last_role, last_messages)) =
                    segments.last_mut()
                    && *last_role == progress_role
                {
                    last_messages.extend(messages);
                } else {
                    segments.push(OrderedActiveSegment::Progress(progress_role, messages));
                }
            }
            "Agent" => {
                if !exploration_items.is_empty() {
                    saw_interleaving = true;
                    flush_exploration(&mut segments, &mut exploration_items);
                }
                segments.push(OrderedActiveSegment::Agent(entry.message.as_str()));
            }
            "Tool Result" | "Tool Error" | "Tool Progress" | "System"
                if !exploration_items.is_empty() =>
            {
                saw_interleaving = true;
                flush_exploration(&mut segments, &mut exploration_items);
            }
            _ => {}
        }
    }

    flush_exploration(&mut segments, &mut exploration_items);

    let simple_exploration_then_agent = segments.len() == 2
        && matches!(segments.first(), Some(OrderedActiveSegment::Exploration(_)))
        && matches!(segments.last(), Some(OrderedActiveSegment::Agent(_)));

    if saw_interleaving
        || (segments.len() > 1 && !simple_exploration_then_agent)
        || (force_ordered_agent && !segments.is_empty())
    {
        Some(segments)
    } else {
        None
    }
}
