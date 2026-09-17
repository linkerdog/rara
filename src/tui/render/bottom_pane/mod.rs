mod activity;
pub(super) mod composer;
mod footer;
mod interaction;
mod layout;
mod view;
mod view_builder;

pub(super) use layout::{bottom_pane_style, render_bottom_pane};
pub(crate) use layout::{desired_bottom_pane_height, desired_viewport_height};
