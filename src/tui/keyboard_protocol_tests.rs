use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::tui::app_event::AppEvent;
use crate::tui::event_stream::{UiEvent, translate_event};
use crate::tui::testing::TuiHarness;

enum Action {
    Newline,
    Submit,
    Text(char),
    Ignore,
    Suspend,
}

struct Case {
    bytes: &'static [u8],
    key: KeyEvent,
    action: Action,
}

fn cases(scenario: &str) -> Vec<Case> {
    use Action::*;
    use KeyCode::*;
    use KeyEventKind::*;
    use KeyModifiers as Mods;

    let case = |bytes, code, modifiers, kind, action| Case {
        bytes,
        key: KeyEvent::new_with_kind(code, modifiers, kind),
        action,
    };
    match scenario {
        "enhanced" => vec![
            case(b"\x1b[13;2u", Enter, Mods::SHIFT, Press, Newline),
            case(b"\r", Enter, Mods::NONE, Press, Submit),
            case(b"\x1b[13;5u", Enter, Mods::CONTROL, Press, Submit),
            case(b"\x1b[106;5u", Char('j'), Mods::CONTROL, Press, Newline),
            case(b"a", Char('a'), Mods::NONE, Press, Text('a')),
            case(b"\x1b[97;1:2u", Char('a'), Mods::NONE, Repeat, Text('a')),
            case(b"\x1b[97;1:3u", Char('a'), Mods::NONE, Release, Ignore),
            case(b"\x1b[13;2:3u", Enter, Mods::SHIFT, Release, Ignore),
            case(b"\x1b[99;5:2u", Char('c'), Mods::CONTROL, Repeat, Ignore),
            case(b"\x1b[100;5:2u", Char('d'), Mods::CONTROL, Repeat, Ignore),
            case(b"\x1b[122;5:2u", Char('z'), Mods::CONTROL, Repeat, Ignore),
            case(b"\x1b[122;5u", Char('z'), Mods::CONTROL, Press, Suspend),
        ],
        "legacy" => vec![
            case(b"\r", Enter, Mods::NONE, Press, Submit),
            case(b"\n", Char('j'), Mods::CONTROL, Press, Newline),
            case(b"a", Char('a'), Mods::NONE, Press, Text('a')),
        ],
        other => panic!("unknown keyboard scenario: {other}"),
    }
}

pub(super) fn input(scenario: &str) -> Vec<u8> {
    cases(scenario)
        .into_iter()
        .flat_map(|case| case.bytes.iter().copied())
        .collect()
}

pub(super) fn check_input(scenario: &str) {
    let mut tui = TuiHarness::new(Default::default()).expect("harness");
    for case in cases(scenario) {
        assert!(event::poll(Duration::from_secs(2)).expect("poll keyboard input"));
        let decoded = event::read().expect("decode keyboard input");
        assert_eq!(decoded, Event::Key(case.key));
        let translated = translate_event(decoded, tui.app_mut());
        match case.action {
            Action::Newline => assert!(matches!(
                translated,
                Some(UiEvent::App(AppEvent::InsertNewline))
            )),
            Action::Submit => assert!(matches!(
                translated,
                Some(UiEvent::App(AppEvent::SubmitComposer))
            )),
            Action::Text(expected) => assert!(matches!(
                translated,
                Some(UiEvent::App(AppEvent::InputChar(actual))) if actual == expected
            )),
            Action::Ignore => assert!(translated.is_none()),
            Action::Suspend => assert!(matches!(translated, Some(UiEvent::Suspend))),
        }
    }
    tui.expect_no_commands();
}
