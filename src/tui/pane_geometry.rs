const SIDEBAR_WIDTH: u16 = 38;
const MIN_SIDEBAR_TERMINAL_WIDTH: u16 = 121;

pub(crate) struct PaneColumns {
    pub terminal_width: u16,
    pub sidebar_visible: bool,
}

impl PaneColumns {
    pub(crate) fn sidebar_width(&self) -> Option<u16> {
        (self.terminal_width >= MIN_SIDEBAR_TERMINAL_WIDTH && self.sidebar_visible)
            .then_some(SIDEBAR_WIDTH)
    }

    pub(crate) fn main_width(&self) -> u16 {
        self.terminal_width
            .saturating_sub(self.sidebar_width().unwrap_or(0))
    }
}
