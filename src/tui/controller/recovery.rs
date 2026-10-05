use super::{QueryReceiptBoundary, TuiController};
use crate::runtime_control::RuntimeControlEvent;
use crate::runtime_event_bus::RuntimeReplayGap;
use crate::tui::runtime::RuntimeCommandProcessor;
use crate::tui::runtime_port::RuntimeProjectionEvent;
use crate::tui::state::NoticeLevel;

impl TuiController {
    pub(super) fn drain_query_receipts(&mut self, boundary: QueryReceiptBoundary) -> bool {
        let mut changed = false;
        while let Some(event) = self.next_query_receipt(boundary) {
            changed |= self.project_runtime_event(RuntimeProjectionEvent::Runtime(event));
        }
        changed
    }

    pub(super) fn recover_event_gap(&mut self, gap: RuntimeReplayGap) -> bool {
        let events = self.collect_query_receipts(QueryReceiptBoundary::BroadcastSequence(
            gap.oldest_available.saturating_sub(1),
        ));
        let mut changed = self.report_missing_events(gap, &events);
        for event in events {
            changed |= self.project_runtime_event(RuntimeProjectionEvent::Runtime(Box::new(event)));
        }
        self.runtime_cursor = self
            .runtime_cursor
            .max(gap.oldest_available.saturating_sub(1));
        changed
    }

    fn report_missing_events(
        &mut self,
        gap: RuntimeReplayGap,
        receipts: &[RuntimeControlEvent],
    ) -> bool {
        let applied = self.runtime_cursor;
        let mut expected = gap.requested.max(applied).saturating_add(1);
        for event in receipts {
            if event.sequence < expected {
                continue;
            }
            if event.sequence != expected {
                break;
            }
            expected = expected.saturating_add(1);
        }
        if expected >= gap.oldest_available {
            return false;
        }
        let already_pending = self.runtime_resync_through.is_some();
        self.runtime_resync_through = Some(
            self.runtime_resync_through
                .map_or(gap.latest, |through| through.max(gap.latest)),
        );
        if already_pending {
            log::debug!(
                "Runtime recovery still pending: also missing {expected}..{}",
                gap.oldest_available - 1
            );
            return false;
        }
        log::warn!(
            "Runtime event replay exhausted: missing {expected}..{}",
            gap.oldest_available - 1
        );
        self.app.push_notice(NoticeLevel::Warning, format!(
            "Some runtime events in {expected}..{} could not be recovered. Refreshing current state; some output may be missing.",
            gap.oldest_available - 1,
        ));
        self.needs_redraw = true;
        true
    }

    pub(in crate::tui) fn resync_after_event_loss(
        &mut self,
        processor: &mut RuntimeCommandProcessor,
    ) -> bool {
        let Some(through) = self.runtime_resync_through else {
            return false;
        };
        let goal = self.app.goal_handle.snapshot();
        let mut changed = self.app.goal != goal;
        self.app.goal = goal;
        // Apply the retained tail before refreshing, or old events could
        // overwrite the fresh snapshot. A running task must return its agent.
        if self.runtime_cursor < through || processor.agent().is_none() {
            changed |= processor.sync_agent_activity(&mut self.app);
            self.needs_redraw |= changed;
            return changed;
        }
        processor.sync_snapshot(&mut self.app);
        if !self.app.is_busy() {
            self.app.finalize_agent_stream(None);
            self.app.clear_active_live_sections();
            if matches!(
                self.app.runtime_phase,
                crate::tui::state::RuntimePhase::SendingPrompt
                    | crate::tui::state::RuntimePhase::ProcessingResponse
                    | crate::tui::state::RuntimePhase::RunningTool
            ) {
                self.app.set_runtime_phase(
                    crate::tui::state::RuntimePhase::Idle,
                    Some("runtime state refreshed".into()),
                );
            }
        }
        self.runtime_resync_through = None;
        self.publish_snapshot_projection();
        self.needs_redraw = true;
        true
    }
}
