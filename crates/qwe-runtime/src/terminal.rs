use std::num::NonZeroU16;

use vt100::{
    CursorStyle as VtCursorStyle, MouseProtocolEncoding as VtMouseProtocolEncoding,
    MouseProtocolMode as VtMouseProtocolMode, Parser,
};

use crate::TerminalState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseProtocolMode {
    None,
    Press,
    PressRelease,
    ButtonMotion,
    AnyMotion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseProtocolEncoding {
    Default,
    Utf8,
    Sgr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Default,
    BlinkBlock,
    SteadyBlock,
    BlinkUnderline,
    SteadyUnderline,
    BlinkVerticalBar,
    SteadyVerticalBar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub rows: u16,
    pub columns: u16,
    pub cursor_row: u16,
    pub cursor_column: u16,
    pub cursor_visible: bool,
    pub cursor_style: CursorStyle,
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub bracketed_paste: bool,
    pub mouse_protocol_mode: MouseProtocolMode,
    pub mouse_protocol_encoding: MouseProtocolEncoding,
    pub screen_text: String,
}

pub struct TerminalScreen {
    parser: Parser,
}

impl TerminalScreen {
    #[must_use]
    pub fn new(rows: u16, columns: u16, scrollback: usize) -> Self {
        Self {
            parser: Parser::new(
                NonZeroU16::new(rows).expect("terminal rows must be non-zero"),
                NonZeroU16::new(columns).expect("terminal columns must be non-zero"),
                scrollback,
            ),
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, rows: u16, columns: u16) {
        self.parser.set_size(
            NonZeroU16::new(rows).expect("terminal rows must be non-zero"),
            NonZeroU16::new(columns).expect("terminal columns must be non-zero"),
        );
    }

    #[must_use]
    pub fn snapshot(&self) -> TerminalSnapshot {
        let screen = self.parser.screen();
        let (rows, columns) = screen.size();
        let (cursor_row, cursor_column) = screen.cursor_position();
        TerminalSnapshot {
            rows: rows.get(),
            columns: columns.get(),
            cursor_row,
            cursor_column,
            cursor_visible: !screen.hide_cursor(),
            cursor_style: cursor_style(screen.cursor_style()),
            alternate_screen: screen.alternate_screen(),
            application_cursor: screen.application_cursor(),
            application_keypad: screen.application_keypad(),
            bracketed_paste: screen.bracketed_paste(),
            mouse_protocol_mode: mouse_protocol_mode(screen.mouse_protocol_mode()),
            mouse_protocol_encoding: mouse_protocol_encoding(screen.mouse_protocol_encoding()),
            screen_text: screen.contents(),
        }
    }
}

impl TerminalState for TerminalScreen {
    fn process(&mut self, bytes: &[u8]) {
        self.process(bytes);
    }

    fn snapshot(&self) -> Vec<u8> {
        self.snapshot().screen_text.into_bytes()
    }
}

fn cursor_style(style: VtCursorStyle) -> CursorStyle {
    match style {
        VtCursorStyle::Default => CursorStyle::Default,
        VtCursorStyle::BlinkBlock => CursorStyle::BlinkBlock,
        VtCursorStyle::SteadyBlock => CursorStyle::SteadyBlock,
        VtCursorStyle::BlinkUnderline => CursorStyle::BlinkUnderline,
        VtCursorStyle::SteadyUnderline => CursorStyle::SteadyUnderline,
        VtCursorStyle::BlinkVerticalBar => CursorStyle::BlinkVerticalBar,
        VtCursorStyle::SteadyVerticalBar => CursorStyle::SteadyVerticalBar,
    }
}

fn mouse_protocol_mode(mode: VtMouseProtocolMode) -> MouseProtocolMode {
    match mode {
        VtMouseProtocolMode::None => MouseProtocolMode::None,
        VtMouseProtocolMode::Press => MouseProtocolMode::Press,
        VtMouseProtocolMode::PressRelease => MouseProtocolMode::PressRelease,
        VtMouseProtocolMode::ButtonMotion => MouseProtocolMode::ButtonMotion,
        VtMouseProtocolMode::AnyMotion => MouseProtocolMode::AnyMotion,
    }
}

fn mouse_protocol_encoding(encoding: VtMouseProtocolEncoding) -> MouseProtocolEncoding {
    match encoding {
        VtMouseProtocolEncoding::Default => MouseProtocolEncoding::Default,
        VtMouseProtocolEncoding::Utf8 => MouseProtocolEncoding::Utf8,
        VtMouseProtocolEncoding::Sgr => MouseProtocolEncoding::Sgr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_includes_cursor_modes_and_alternate_screen() {
        let mut terminal = TerminalScreen::new(10, 20, 100);
        terminal.process(b"primary");
        terminal.process(b"\x1b[?1049h\x1b[2;3H\x1b[?25l\x1b[2 q");
        terminal.process(b"\x1b=\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h");

        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.rows, 10);
        assert_eq!(snapshot.columns, 20);
        assert_eq!((snapshot.cursor_row, snapshot.cursor_column), (1, 2));
        assert!(!snapshot.cursor_visible);
        assert_eq!(snapshot.cursor_style, CursorStyle::SteadyBlock);
        assert!(snapshot.alternate_screen);
        assert!(snapshot.application_cursor);
        assert!(snapshot.application_keypad);
        assert!(snapshot.bracketed_paste);
        assert_eq!(
            snapshot.mouse_protocol_mode,
            MouseProtocolMode::ButtonMotion
        );
        assert_eq!(snapshot.mouse_protocol_encoding, MouseProtocolEncoding::Sgr);
    }

    #[test]
    fn resize_updates_structured_dimensions() {
        let mut terminal = TerminalScreen::new(24, 80, 100);
        terminal.process(b"resize");
        terminal.resize(40, 120);
        assert_eq!(terminal.snapshot().rows, 40);
        assert_eq!(terminal.snapshot().columns, 120);
    }
}
