use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Rect, Size};

pub(crate) struct EmulatedScreen {
    pub parser: vt100::Parser,
    pub output: Vec<u8>,
    pub fail_next_write: bool,
}

struct EmulatorWriter(Rc<RefCell<EmulatedScreen>>);

impl Write for EmulatorWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut screen = self.0.borrow_mut();
        if std::mem::take(&mut screen.fail_next_write) {
            return Err(io::Error::other("injected terminal write failure"));
        }
        screen.output.extend_from_slice(bytes);
        screen.parser.process(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Run production ANSI writes through a screen model without querying a real TTY.
pub(crate) struct EmulatorBackend {
    pub screen: Rc<RefCell<EmulatedScreen>>,
    pub fail_cursor_query: bool,
    pub cursor_queries: usize,
    backend: CrosstermBackend<EmulatorWriter>,
}

impl EmulatorBackend {
    pub fn new(rows: u16, columns: u16) -> Self {
        let screen = Rc::new(RefCell::new(EmulatedScreen {
            parser: vt100::Parser::new(rows, columns, 1000),
            output: Vec::new(),
            fail_next_write: false,
        }));
        Self {
            backend: CrosstermBackend::new(EmulatorWriter(screen.clone())),
            screen,
            fail_cursor_query: false,
            cursor_queries: 0,
        }
    }
}

pub(crate) fn render_app_viewport(
    app: &mut super::super::state::TuiApp,
    width: u16,
    rows: u16,
) -> Rect {
    let backend = EmulatorBackend::new(rows, width);
    let mut terminal =
        super::super::custom_terminal::Terminal::new(backend).expect("emulated terminal");
    terminal
        .draw_inline(|frame| super::super::render::render(frame, app))
        .expect("render inline app");
    terminal.viewport_area
}

impl Write for EmulatorBackend {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Write::write(&mut self.backend, bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.backend)
    }
}

impl Backend for EmulatorBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.backend.draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.backend.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.backend.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.cursor_queries += 1;
        if self.fail_cursor_query {
            return Err(io::Error::other("injected cursor query failure"));
        }
        let (y, x) = self.screen.borrow().parser.screen().cursor_position();
        Ok(Position::new(x, y))
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.backend.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.backend.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.backend.clear_region(clear_type)
    }

    fn append_lines(&mut self, count: u16) -> io::Result<()> {
        self.backend.append_lines(count)
    }

    fn size(&self) -> io::Result<Size> {
        let (height, width) = self.screen.borrow().parser.screen().size();
        Ok(Size { width, height })
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size()?,
            pixels: Size::ZERO,
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.backend)
    }
}
