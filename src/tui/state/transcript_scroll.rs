/// Numeric geometry from the authoritative, prewrapped transcript viewport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TranscriptScrollLayout {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) content_rows: usize,
}

impl TranscriptScrollLayout {
    fn max_offset(self) -> usize {
        // Preserve the transcript's one-row breathing room at the tail.
        let visible_rows = usize::from(self.height.saturating_sub(1).max(1));
        self.content_rows.saturating_sub(visible_rows)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ScrollPosition {
    #[default]
    FollowTail,
    Anchored(usize),
}

/// Owns bounded visual-row navigation without depending on terminal widgets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TranscriptScroll {
    position: ScrollPosition,
    layout: Option<TranscriptScrollLayout>,
}

impl TranscriptScroll {
    pub(crate) fn layout(&self) -> Option<TranscriptScrollLayout> {
        self.layout
    }

    pub(crate) fn update_layout(&mut self, layout: TranscriptScrollLayout) -> usize {
        self.layout = Some(layout);
        self.position = match self.position {
            ScrollPosition::FollowTail => ScrollPosition::FollowTail,
            ScrollPosition::Anchored(top) => ScrollPosition::Anchored(top.min(layout.max_offset())),
        };
        self.offset()
    }

    pub(crate) fn offset(&self) -> usize {
        match self.position {
            ScrollPosition::FollowTail => self.layout.map_or(0, TranscriptScrollLayout::max_offset),
            ScrollPosition::Anchored(top) => top,
        }
    }

    pub(crate) fn follow_tail(&mut self) {
        self.position = ScrollPosition::FollowTail;
    }

    pub(crate) fn scroll(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }
        let max_offset = self.layout.map_or(0, TranscriptScrollLayout::max_offset);
        let offset = if delta < 0 {
            self.offset().saturating_sub(delta.unsigned_abs() as usize)
        } else {
            self.offset().saturating_add(delta as usize).min(max_offset)
        };
        self.position = if offset == max_offset {
            ScrollPosition::FollowTail
        } else {
            ScrollPosition::Anchored(offset)
        };
    }
}

#[cfg(test)]
#[path = "transcript_scroll_tests.rs"]
mod tests;
