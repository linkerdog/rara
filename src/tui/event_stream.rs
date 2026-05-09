use std::sync::Mutex;
use std::time::Instant;

use crossterm::event::{Event, KeyEventKind, MouseEvent, MouseEventKind};

use super::app_event::AppEvent;
use super::state::TuiApp;

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
}

pub fn translate_event(event: Event, app: &TuiApp) -> Option<UiEvent> {
    match event {
        Event::Key(key_event) => {
            if matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                Some(UiEvent::App(super::map_key_to_event(key_event, app)))
            } else {
                None
            }
        }
        Event::Mouse(mouse_event) => Some(UiEvent::App(map_mouse_to_event(mouse_event, app))),
        Event::Resize(_, _) => Some(UiEvent::Draw),
        Event::Paste(text) => Some(UiEvent::Paste(text)),
        Event::FocusGained => Some(UiEvent::FocusChanged(true)),
        Event::FocusLost => Some(UiEvent::FocusChanged(false)),
    }
}

fn map_mouse_to_event(mouse_event: MouseEvent, app: &TuiApp) -> AppEvent {
    if app.overlay.is_some() {
        return AppEvent::Noop;
    }

    match mouse_event.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let direction: i32 = if matches!(mouse_event.kind, MouseEventKind::ScrollUp) {
                -1
            } else {
                1
            };
            let lines = MOUSE_WHEEL_SCROLL_LINES as f64 * scroll_accel_factor();
            AppEvent::ScrollTranscript((direction * lines.round() as i32).clamp(-15, 15))
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
