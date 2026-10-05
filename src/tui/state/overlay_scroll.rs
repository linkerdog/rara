use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub(crate) enum OverlayNavigation {
    Rows(i32),
    PageUp,
    PageDown,
    Start,
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OverlayScrollLayout {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) content_rows: usize,
}

impl OverlayScrollLayout {
    fn max_offset(self) -> usize {
        if self.width == 0 || self.height == 0 {
            return 0;
        }
        self.content_rows.saturating_sub(usize::from(self.height))
    }
}

/// Numeric navigation for a measured, prewrapped overlay body.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct OverlayScroll {
    offset: usize,
    layout: Option<OverlayScrollLayout>,
}

impl OverlayScroll {
    pub(crate) fn layout(&self) -> Option<OverlayScrollLayout> {
        self.layout
    }

    #[cfg(test)]
    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    pub(crate) fn update_layout(&mut self, layout: OverlayScrollLayout) {
        self.layout = Some(layout);
        self.offset = self.offset.min(layout.max_offset());
    }

    pub(crate) fn visible_range(&self) -> Range<usize> {
        let end = self.layout.map_or(0, |layout| {
            if layout.width == 0 {
                return 0;
            }
            self.offset
                .saturating_add(usize::from(layout.height))
                .min(layout.content_rows)
        });
        self.offset..end
    }

    pub(crate) fn navigate(&mut self, navigation: OverlayNavigation) {
        let Some(layout) = self.layout else {
            return;
        };
        let page = usize::from(layout.height.saturating_sub(1).max(1));
        self.offset = match navigation {
            OverlayNavigation::Rows(delta) if delta < 0 => {
                self.offset.saturating_sub(delta.unsigned_abs() as usize)
            }
            OverlayNavigation::Rows(delta) => self.offset.saturating_add(delta as usize),
            OverlayNavigation::PageUp => self.offset.saturating_sub(page),
            OverlayNavigation::PageDown => self.offset.saturating_add(page),
            OverlayNavigation::Start => 0,
            OverlayNavigation::End => layout.max_offset(),
        }
        .min(layout.max_offset());
    }

    pub(crate) fn reveal(&mut self, rows: Range<usize>) {
        let Some(layout) = self.layout else {
            return;
        };
        let height = usize::from(layout.height);
        if rows.start < self.offset || rows.len() > height {
            self.offset = rows.start;
        } else if rows.end > self.offset.saturating_add(height) {
            self.offset = rows.end.saturating_sub(height);
        }
        self.offset = self.offset.min(layout.max_offset());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_clamps_immediately_and_reverses_without_scroll_debt() {
        let mut scroll = OverlayScroll::default();
        scroll.update_layout(OverlayScrollLayout {
            width: 40,
            height: 6,
            content_rows: 30,
        });
        for _ in 0..10 {
            scroll.navigate(OverlayNavigation::Rows(i32::MAX));
        }
        assert_eq!(scroll.visible_range(), 24..30);
        scroll.navigate(OverlayNavigation::Rows(-1));
        assert_eq!(scroll.offset(), 23);
        scroll.navigate(OverlayNavigation::Rows(i32::MIN));
        assert_eq!(scroll.offset(), 0);
        scroll.navigate(OverlayNavigation::Rows(1));
        assert_eq!(scroll.offset(), 1);
    }

    #[test]
    fn pages_use_the_body_height_and_large_offsets_remain_reachable() {
        let mut scroll = OverlayScroll::default();
        scroll.update_layout(OverlayScrollLayout {
            width: 40,
            height: 6,
            content_rows: 80_000,
        });
        scroll.navigate(OverlayNavigation::PageDown);
        assert_eq!(scroll.offset(), 5);
        scroll.navigate(OverlayNavigation::PageUp);
        assert_eq!(scroll.offset(), 0);
        scroll.navigate(OverlayNavigation::End);
        assert_eq!(scroll.visible_range(), 79_994..80_000);
        scroll.navigate(OverlayNavigation::Start);
        assert_eq!(scroll.offset(), 0);
    }

    #[test]
    fn resize_and_content_shrink_clamp_the_anchor() {
        let mut scroll = OverlayScroll::default();
        scroll.update_layout(OverlayScrollLayout {
            width: 40,
            height: 6,
            content_rows: 30,
        });
        scroll.navigate(OverlayNavigation::End);
        scroll.update_layout(OverlayScrollLayout {
            width: 80,
            height: 12,
            content_rows: 16,
        });
        assert_eq!(scroll.visible_range(), 4..16);
        scroll.update_layout(OverlayScrollLayout {
            width: 80,
            height: 12,
            content_rows: 3,
        });
        assert_eq!(scroll.visible_range(), 0..3);
        scroll.navigate(OverlayNavigation::End);
        assert_eq!(scroll.offset(), 0);
    }

    #[test]
    fn unmeasured_empty_and_zero_sized_bodies_do_not_accumulate_scroll() {
        let mut scroll = OverlayScroll::default();
        scroll.navigate(OverlayNavigation::End);
        assert_eq!(scroll, OverlayScroll::default());
        for (width, height, content_rows) in [(0, 8, 30), (40, 0, 30), (40, 8, 0)] {
            scroll.update_layout(OverlayScrollLayout {
                width,
                height,
                content_rows,
            });
            scroll.navigate(OverlayNavigation::Rows(i32::MAX));
            assert_eq!(scroll.offset(), 0);
        }
    }

    #[test]
    fn entry_reveal_handles_wrapped_and_taller_than_viewport_entries() {
        let mut scroll = OverlayScroll::default();
        scroll.update_layout(OverlayScrollLayout {
            width: 40,
            height: 6,
            content_rows: 30,
        });
        scroll.reveal(10..14);
        assert_eq!(scroll.visible_range(), 8..14);
        scroll.reveal(2..5);
        assert_eq!(scroll.offset(), 2);
        scroll.reveal(15..25);
        assert_eq!(scroll.offset(), 15);
        assert_eq!(scroll.layout().expect("measured").height, 6);
    }
}
