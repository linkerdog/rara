pub(crate) struct PaneColumns {
    pub terminal_width: u16,
    pub sidebar_visible: bool,
}

impl PaneColumns {
    pub(crate) fn sidebar_width(&self) -> Option<u16> {
        (self.terminal_width > 120 && self.sidebar_visible).then_some(38)
    }

    pub(crate) fn main_width(&self) -> u16 {
        self.terminal_width
            .saturating_sub(self.sidebar_width().unwrap_or(0))
    }
}
