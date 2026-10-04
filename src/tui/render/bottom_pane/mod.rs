mod activity;
pub(super) mod composer;
mod footer;
mod interaction;
mod layout;
mod shell_details;
mod view;
mod view_builder;

pub(crate) use layout::desired_bottom_pane_height;
pub(super) use layout::{bottom_pane_style, render_bottom_pane};
