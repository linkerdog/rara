use super::{ScrollPosition, TranscriptScroll, TranscriptScrollLayout};

fn layout(content_rows: usize, height: u16) -> TranscriptScrollLayout {
    TranscriptScrollLayout {
        width: 80,
        height,
        content_rows,
    }
}

#[test]
fn every_delta_is_clamped_without_accumulating_scroll_debt() {
    let mut scroll = TranscriptScroll::default();
    assert_eq!(scroll.update_layout(layout(100, 11)), 90);
    for _ in 0..100 {
        scroll.scroll(-8);
    }
    assert_eq!(scroll.offset(), 0);
    scroll.scroll(1);
    assert_eq!(scroll.offset(), 1);
    scroll.scroll(i32::MAX);
    assert_eq!(scroll.position, ScrollPosition::FollowTail);
    assert_eq!(scroll.offset(), 90);
    scroll.scroll(i32::MIN);
    assert_eq!(scroll.offset(), 0);
}

#[test]
fn empty_short_and_unmeasured_content_remain_at_zero() {
    let mut scroll = TranscriptScroll::default();
    scroll.scroll(i32::MIN);
    assert_eq!(scroll, TranscriptScroll::default());
    for rows in [0, 1, 9, 10] {
        assert_eq!(scroll.update_layout(layout(rows, 11)), 0);
        scroll.scroll(-8);
        scroll.scroll(8);
        assert_eq!(scroll.offset(), 0);
        assert_eq!(scroll.position, ScrollPosition::FollowTail);
    }
}

#[test]
fn append_follows_only_when_tail_following_is_active() {
    let mut scroll = TranscriptScroll::default();
    assert_eq!(scroll.update_layout(layout(20, 6)), 15);
    assert_eq!(scroll.update_layout(layout(30, 6)), 25);
    scroll.scroll(-8);
    assert_eq!(scroll.position, ScrollPosition::Anchored(17));
    assert_eq!(scroll.update_layout(layout(50, 6)), 17);
    scroll.scroll(i32::MAX);
    assert_eq!(scroll.position, ScrollPosition::FollowTail);
    assert_eq!(scroll.update_layout(layout(60, 6)), 55);
}

#[test]
fn layout_clamps_an_anchor_without_implicitly_following_new_content() {
    let mut scroll = TranscriptScroll::default();
    scroll.update_layout(layout(100, 11));
    scroll.scroll(-20);
    assert_eq!(scroll.offset(), 70);
    assert_eq!(scroll.update_layout(layout(80, 11)), 70);
    scroll.scroll(0);
    assert_eq!(scroll.position, ScrollPosition::Anchored(70));
    assert_eq!(scroll.update_layout(layout(100, 11)), 70);
    assert_eq!(scroll.update_layout(layout(20, 11)), 10);
    assert_eq!(scroll.position, ScrollPosition::Anchored(10));
    assert_eq!(scroll.update_layout(layout(50, 41)), 10);
    assert_eq!(scroll.update_layout(layout(50, 11)), 10);
    scroll.follow_tail();
    assert_eq!(scroll.offset(), 40);
}

#[test]
fn visual_row_offsets_are_not_limited_to_terminal_coordinate_width() {
    let mut scroll = TranscriptScroll::default();
    assert_eq!(scroll.update_layout(layout(70_000, 20)), 69_981);
    scroll.scroll(-4_000);
    assert_eq!(scroll.offset(), 65_981);
    assert_eq!(scroll.update_layout(layout(80_000, 20)), 65_981);
    scroll.scroll(i32::MAX);
    assert_eq!(scroll.offset(), 79_981);
}
