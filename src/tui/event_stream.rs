use std::sync::Mutex;
use std::time::Instant;

use crossterm::event::{
    Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::app_event::AppEvent;
use super::selection::ScreenPosition;
use super::state::{Overlay, TuiApp};

const MOUSE_WHEEL_SCROLL_LINES: i32 = 3;
/// Maximum acceleration multiplier for rapid scrolling.
const SCROLL_ACCEL_MAX: f64 = 5.0;
/// Time threshold (ms) below which acceleration kicks in.
const SCROLL_ACCEL_THRESHOLD_MS: u128 = 50;
/// Time threshold (ms) above which acceleration resets.
const SCROLL_ACCEL_RESET_MS: u128 = 150;

static LAST_SCROLL_EVENT: Mutex<Option<Instant>> = Mutex::new(None);
static SCROLL_VELOCITY: Mutex<f64> = Mutex::new(1.0);

#[derive(Debug)]
pub enum UiEvent {
    App(AppEvent),
    Draw,
    Paste(String),
    FocusChanged(bool),
    #[cfg(unix)]
    Suspend,
}

pub fn translate_event(event: Event, app: &mut TuiApp) -> Option<UiEvent> {
    match event {
        Event::Key(key_event) => {
            if matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                let control = key_event.modifiers == KeyModifiers::CONTROL;
                if control
                    && matches!(key_event.code, KeyCode::Char('c' | 'd' | 'z'))
                    && key_event.kind == KeyEventKind::Repeat
                {
                    return None;
                }
                if control && key_event.code == KeyCode::Char('z') {
                    app.quit_shortcut.clear();
                    #[cfg(unix)]
                    return Some(UiEvent::Suspend);
                    #[cfg(not(unix))]
                    return Some(UiEvent::App(AppEvent::Noop));
                }
                // Flushing can hide the palette before dismissal intent is routed.
                let discarding_palette = matches!(app.overlay, Some(Overlay::CommandPalette))
                    && (key_event.code == KeyCode::Esc
                        || (control && key_event.code == KeyCode::Char('c')));
                if app.composer_input_is_active() && !discarding_palette {
                    app.flush_composer_paste();
                }
                Some(UiEvent::App(super::map_key_to_event(key_event, app)))
            } else {
                None
            }
        }
        Event::Mouse(mouse_event) => {
            app.quit_shortcut.clear();
            Some(UiEvent::App(map_mouse_to_event(mouse_event, app)))
        }
        Event::Resize(_, _) => Some(UiEvent::Draw),
        Event::Paste(text) => {
            app.quit_shortcut.clear();
            Some(UiEvent::Paste(text))
        }
        Event::FocusGained | Event::FocusLost => {
            let focused = matches!(event, Event::FocusGained);
            app.terminal_focused = focused;
            Some(UiEvent::FocusChanged(focused))
        }
    }
}

#[cfg(test)]
#[test]
fn review_regression_terminal_focus_updates_display_state() {
    let mut harness = super::testing::TuiHarness::new(Default::default()).expect("harness");
    translate_event(Event::FocusLost, harness.app_mut());
    assert!(!harness.app().terminal_focused);
    assert!(!harness.app().terminal_diagnostics_view().focused);
    translate_event(Event::FocusGained, harness.app_mut());
    assert!(harness.app().terminal_focused);
    assert!(harness.app().terminal_diagnostics_view().focused);
}

fn map_mouse_to_event(mouse_event: MouseEvent, app: &TuiApp) -> AppEvent {
    match mouse_event.kind {
        MouseEventKind::Down(MouseButton::Left) if app.overlay.is_none() => {
            AppEvent::StartTranscriptSelection(ScreenPosition::new(
                mouse_event.column,
                mouse_event.row,
            ))
        }
        MouseEventKind::Drag(MouseButton::Left) if app.overlay.is_none() => {
            AppEvent::DragTranscriptSelection(ScreenPosition::new(
                mouse_event.column,
                mouse_event.row,
            ))
        }
        MouseEventKind::Up(MouseButton::Left) if app.overlay.is_none() => {
            AppEvent::FinishTranscriptSelection(ScreenPosition::new(
                mouse_event.column,
                mouse_event.row,
            ))
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if app.transcript_selection.is_dragging() {
                return AppEvent::Noop;
            }
            let direction: i32 = if matches!(mouse_event.kind, MouseEventKind::ScrollUp) {
                -1
            } else {
                1
            };
            let lines = MOUSE_WHEEL_SCROLL_LINES as f64 * scroll_accel_factor();
            let delta = (direction * lines.round() as i32).clamp(-15, 15);
            match &app.overlay {
                Some(Overlay::Context) => AppEvent::ScrollContext(delta),
                Some(Overlay::CommandPalette) | Some(Overlay::ModelSearch) => {
                    AppEvent::MoveCommandSelection(delta)
                }
                Some(Overlay::ListPicker(_)) => AppEvent::MoveListPickerSelection(delta),
                Some(Overlay::PermissionPicker) => AppEvent::MovePermissionSelection(delta),
                Some(Overlay::SkillsPicker) => AppEvent::MoveSkillsSelection(delta),
                Some(_) => AppEvent::Noop,
                None => AppEvent::ScrollTranscript(delta),
            }
        }
        _ => AppEvent::Noop,
    }
}

fn scroll_accel_factor() -> f64 {
    let now = Instant::now();
    let mut last = LAST_SCROLL_EVENT.lock().unwrap();
    let mut velocity = SCROLL_VELOCITY.lock().unwrap();

    let factor = if let Some(prev) = *last {
        let elapsed_ms = now.duration_since(prev).as_millis();
        if elapsed_ms < SCROLL_ACCEL_THRESHOLD_MS {
            *velocity = (*velocity + 0.8).min(SCROLL_ACCEL_MAX);
        } else if elapsed_ms > SCROLL_ACCEL_RESET_MS {
            *velocity = 1.0;
        }
        // else: keep current velocity (coasting)
        *velocity
    } else {
        1.0
    };

    *last = Some(now);
    factor
}
