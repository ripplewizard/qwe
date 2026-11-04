use std::num::NonZeroU16;
use vt100::{CursorStyle, MouseProtocolEncoding, MouseProtocolMode, Parser};

fn dimensions(rows: u16, columns: u16) -> (NonZeroU16, NonZeroU16) {
    (
        NonZeroU16::new(rows).expect("rows are non-zero"),
        NonZeroU16::new(columns).expect("columns are non-zero"),
    )
}

fn parser(rows: u16, columns: u16) -> Parser {
    let (rows, columns) = dimensions(rows, columns);
    Parser::new(rows, columns, 100)
}

#[test]
fn cursor_position_visibility_and_style_are_observable() {
    let mut parser = parser(10, 20);
    parser.process(b"\x1b[3;4H\x1b[?25l\x1b[2 q");
    let screen = parser.screen();
    assert_eq!(screen.cursor_position(), (2, 3));
    assert!(screen.hide_cursor());
    assert_eq!(screen.cursor_style(), CursorStyle::SteadyBlock);
}

#[test]
fn interactive_terminal_modes_are_observable_and_reset() {
    let mut parser = parser(10, 20);
    parser.process(b"\x1b=\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h");
    let screen = parser.screen();
    assert!(screen.application_keypad());
    assert!(screen.application_cursor());
    assert!(screen.bracketed_paste());
    assert_eq!(
        screen.mouse_protocol_mode(),
        MouseProtocolMode::ButtonMotion
    );
    assert_eq!(screen.mouse_protocol_encoding(), MouseProtocolEncoding::Sgr);

    parser.process(b"\x1b>\x1b[?1l\x1b[?2004l\x1b[?1002l\x1b[?1006l");
    let screen = parser.screen();
    assert!(!screen.application_keypad());
    assert!(!screen.application_cursor());
    assert!(!screen.bracketed_paste());
    assert_eq!(screen.mouse_protocol_mode(), MouseProtocolMode::None);
    assert_eq!(
        screen.mouse_protocol_encoding(),
        MouseProtocolEncoding::Default
    );
}

#[test]
fn alternate_screen_switches_and_restores_primary_contents() {
    let mut parser = parser(5, 20);
    parser.process(b"primary");
    parser.process(b"\x1b[?1049h\x1b[Halt");
    assert!(parser.screen().alternate_screen());
    assert!(parser.screen().contents().contains("alt"));
    assert!(!parser.screen().contents().contains("primary"));

    parser.process(b"\x1b[?1049l");
    assert!(!parser.screen().alternate_screen());
    assert!(parser.screen().contents().contains("primary"));
}

#[test]
fn wide_characters_occupy_two_cells() {
    let mut parser = parser(3, 8);
    parser.process("界x".as_bytes());
    assert_eq!(parser.screen().cursor_position(), (0, 3));
    let first = parser.screen().cell(0, 0).expect("first cell exists");
    let continuation = parser.screen().cell(0, 1).expect("continuation exists");
    assert!(first.is_wide());
    assert!(continuation.is_wide_continuation());
}

#[test]
fn width_resize_is_naive_truncation_without_reflow() {
    let mut parser = parser(3, 8);
    parser.process(b"abcdef");
    let (rows, columns) = dimensions(3, 3);
    parser.set_size(rows, columns);
    assert_eq!(parser.screen().contents().trim_end(), "abc");
    assert_eq!(parser.screen().scrollback_len(), 0);
}
