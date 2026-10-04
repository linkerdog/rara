use std::ops::Range;

#[derive(Debug, Clone, Copy)]
pub(crate) enum ApprovalDetailNavigation {
    PageUp,
    PageDown,
    Start,
    End,
}

#[derive(Debug, Default)]
pub(crate) struct ApprovalDetailScroll {
    request_id: Option<String>,
    offset: usize,
    max_offset: usize,
    page_rows: usize,
}

impl ApprovalDetailScroll {
    fn sync_request(&mut self, request_id: &str) {
        if self.request_id.as_deref() != Some(request_id) {
            *self = Self {
                request_id: Some(request_id.to_owned()),
                ..Self::default()
            };
        }
    }

    pub(crate) fn visible_range(
        &mut self,
        request_id: &str,
        total_rows: usize,
        visible_rows: usize,
    ) -> Range<usize> {
        self.sync_request(request_id);
        self.page_rows = visible_rows.max(1);
        self.max_offset = total_rows.saturating_sub(self.page_rows);
        self.offset = self.offset.min(self.max_offset);
        self.offset..self.offset.saturating_add(visible_rows).min(total_rows)
    }

    pub(crate) fn navigate(&mut self, request_id: &str, direction: ApprovalDetailNavigation) {
        self.sync_request(request_id);
        let page = self.page_rows.saturating_sub(1).max(1);
        self.offset = match direction {
            ApprovalDetailNavigation::PageUp => self.offset.saturating_sub(page),
            ApprovalDetailNavigation::PageDown => {
                self.offset.saturating_add(page).min(self.max_offset)
            }
            ApprovalDetailNavigation::Start => 0,
            ApprovalDetailNavigation::End => self.max_offset,
        };
    }
}
