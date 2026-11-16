const MAX_CSI_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Charset(u8),
    Hash,
    Percent,
    Csi,
    IgnoreCsi,
    Osc,
    String,
    StringEscape,
}

#[derive(Debug)]
pub struct RestrictedTerminalFilter {
    state: State,
    csi: Vec<u8>,
    utf8: Vec<u8>,
    utf8_remaining: u8,
}

impl Default for RestrictedTerminalFilter {
    fn default() -> Self {
        Self {
            state: State::Ground,
            csi: Vec::new(),
            utf8: Vec::new(),
            utf8_remaining: 0,
        }
    }
}

impl RestrictedTerminalFilter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut filtered = Vec::with_capacity(bytes.len());
        for byte in bytes.iter().copied() {
            match self.state {
                State::Ground => self.ground(byte, &mut filtered),
                State::Escape => self.escape(byte, &mut filtered),
                State::Charset(intermediate) => {
                    if (0x20..=0x7e).contains(&byte) {
                        filtered.extend_from_slice(&[0x1b, intermediate, byte]);
                    }
                    self.state = State::Ground;
                }
                State::Hash => {
                    if matches!(byte, b'3'..=b'8') {
                        filtered.extend_from_slice(&[0x1b, b'#', byte]);
                    }
                    self.state = State::Ground;
                }
                State::Percent => {
                    if matches!(byte, b'@' | b'8' | b'G') {
                        filtered.extend_from_slice(&[0x1b, b'%', byte]);
                    }
                    self.state = State::Ground;
                }
                State::Csi => self.csi(byte, &mut filtered),
                State::IgnoreCsi => {
                    if is_csi_final(byte) {
                        self.state = State::Ground;
                    }
                }
                State::Osc => {
                    if byte == 0x07 {
                        self.state = State::Ground;
                    } else if byte == 0x1b {
                        self.state = State::StringEscape;
                    }
                }
                State::String => {
                    if byte == 0x1b {
                        self.state = State::StringEscape;
                    }
                }
                State::StringEscape => {
                    self.state = if byte == b'\\' {
                        State::Ground
                    } else {
                        State::String
                    };
                }
            }
        }
        filtered
    }

    pub fn finish(&mut self) -> Vec<u8> {
        self.state = State::Ground;
        self.csi.clear();
        self.utf8.clear();
        self.utf8_remaining = 0;
        Vec::new()
    }

    fn ground(&mut self, byte: u8, filtered: &mut Vec<u8>) {
        if self.utf8_remaining > 0 {
            if (0x80..=0xbf).contains(&byte) {
                self.utf8.push(byte);
                self.utf8_remaining -= 1;
                if self.utf8_remaining == 0 {
                    filtered.extend_from_slice(&self.utf8);
                    self.utf8.clear();
                }
                return;
            }
            self.utf8.clear();
            self.utf8_remaining = 0;
            self.ground(byte, filtered);
            return;
        }
        match byte {
            0x1b => self.state = State::Escape,
            0xc2..=0xdf => {
                self.utf8.push(byte);
                self.utf8_remaining = 1;
            }
            0xe0..=0xef => {
                self.utf8.push(byte);
                self.utf8_remaining = 2;
            }
            0xf0..=0xf4 => {
                self.utf8.push(byte);
                self.utf8_remaining = 3;
            }
            value if display_control(value) => filtered.push(value),
            value if value < 0x20 || value == 0x7f => {}
            0x80..=0xc1 | 0xf5..=0xff => {}
            value => filtered.push(value),
        }
    }

    fn escape(&mut self, byte: u8, filtered: &mut Vec<u8>) {
        match byte {
            b'[' => {
                self.csi.clear();
                self.csi.extend_from_slice(b"\x1b[");
                self.state = State::Csi;
            }
            b']' => self.state = State::Osc,
            b'P' | b'X' | b'^' | b'_' => self.state = State::String,
            b'(' | b')' | b'*' | b'+' => self.state = State::Charset(byte),
            b'#' => self.state = State::Hash,
            b'%' => self.state = State::Percent,
            b'6' | b'7' | b'8' | b'9' | b'=' | b'>' | b'D' | b'E' | b'H' | b'M' => {
                filtered.extend_from_slice(&[0x1b, byte]);
                self.state = State::Ground;
            }
            0x1b => self.state = State::Escape,
            _ => self.state = State::Ground,
        }
    }

    fn csi(&mut self, byte: u8, filtered: &mut Vec<u8>) {
        if is_csi_final(byte) {
            self.csi.push(byte);
            if allowed_csi(&self.csi) {
                filtered.extend_from_slice(&self.csi);
            }
            self.csi.clear();
            self.state = State::Ground;
            return;
        }
        if self.csi.len() >= MAX_CSI_BYTES {
            self.csi.clear();
            self.state = State::IgnoreCsi;
            return;
        }
        self.csi.push(byte);
    }
}

fn display_control(byte: u8) -> bool {
    matches!(byte, 0x07..=0x0f)
}

fn is_csi_final(byte: u8) -> bool {
    (0x40..=0x7e).contains(&byte)
}

fn allowed_csi(sequence: &[u8]) -> bool {
    let Some(final_byte) = sequence.last().copied() else {
        return false;
    };
    if sequence.len() < 3 || &sequence[..2] != b"\x1b[" {
        return false;
    }
    let body = &sequence[2..sequence.len() - 1];
    match final_byte {
        b'A' | b'B' | b'C' | b'D' | b'E' | b'F' | b'G' | b'H' | b'I' | b'J' | b'K' | b'L'
        | b'M' | b'P' | b'S' | b'T' | b'X' | b'Z' | b'@' | b'`' | b'd' | b'e' | b'f' | b'g'
        | b'm' | b'r' | b's' | b'u' => csi_parameters(body) && !body.contains(&b'$'),
        b'q' => body == b" " || csi_cursor_style(body),
        b'h' | b'l' => allowed_private_modes(body),
        _ => false,
    }
}

fn csi_parameters(body: &[u8]) -> bool {
    body.iter()
        .all(|byte| matches!(byte, b'0'..=b'9' | b';' | b':'))
}

fn csi_cursor_style(body: &[u8]) -> bool {
    let Some(mode) = body.strip_suffix(b" ") else {
        return false;
    };
    !mode.is_empty() && mode.iter().all(u8::is_ascii_digit)
}

fn allowed_private_modes(body: &[u8]) -> bool {
    let Some(modes) = body.strip_prefix(b"?") else {
        return false;
    };
    if modes.is_empty() {
        return false;
    }
    modes.split(|byte| *byte == b';').all(|mode| {
        std::str::from_utf8(mode)
            .ok()
            .and_then(|mode| mode.parse::<u16>().ok())
            .is_some_and(|mode| {
                matches!(
                    mode,
                    1 | 4
                        | 6
                        | 7
                        | 12
                        | 25
                        | 47
                        | 1000
                        | 1002
                        | 1003
                        | 1004
                        | 1005
                        | 1006
                        | 1015
                        | 1047
                        | 1049
                        | 2004
                )
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filtered(chunks: &[&[u8]]) -> Vec<u8> {
        let mut filter = RestrictedTerminalFilter::new();
        chunks.iter().flat_map(|chunk| filter.push(chunk)).collect()
    }

    #[test]
    fn preserves_display_sequences_across_chunks() {
        assert_eq!(
            filtered(&[
                b"hello \x1b[31",
                b"mred\x1b[0m\r\n",
                b"\x1b[?1049h",
                b"\x1bM\x1b(0"
            ]),
            b"hello \x1b[31mred\x1b[0m\r\n\x1b[?1049h\x1bM\x1b(0"
        );
    }

    #[test]
    fn drops_osc_and_string_controls() {
        assert_eq!(
            filtered(&[
                b"a\x1b]0,TITLE\x07b",
                b"\x1b]52;c;Y2xpcGJvYXJk\x1b\\c",
                b"\x1bPpayload\x1b\\d"
            ]),
            b"abcd"
        );
    }

    #[test]
    fn drops_queries_unknown_csi_and_malformed_escapes() {
        assert_eq!(
            filtered(&[
                b"\x1b[6n",
                b"\x1b[21t",
                b"\x1b[c",
                b"\x1b[?9001h",
                b"\x1b[>4;2m",
                b"\x1b[?25h",
                b"\x1bZ",
                b"safe",
            ]),
            b"\x1b[?25hsafe"
        );
    }

    #[test]
    fn bounds_incomplete_csi_and_ignores_until_final() {
        let mut input = vec![0x1b, b'['];
        input.extend(std::iter::repeat_n(b'1', MAX_CSI_BYTES + 1));
        input.push(b'm');
        input.extend_from_slice(b"after");
        assert_eq!(filtered(&[&input]), b"after");
    }

    #[test]
    fn preserves_multibyte_utf8_and_standard_controls() {
        assert_eq!(
            filtered(&["终".as_bytes(), "端".as_bytes(), b"\x07\x08\t\r\n"]),
            ["终端".as_bytes(), b"\x07\x08\t\r\n"].concat()
        );
    }

    #[test]
    fn drops_invalid_and_eight_bit_c1_controls() {
        assert_eq!(
            filtered(&[b"a\x9bb\x9bc\x7f", "界".as_bytes(), b"d"]),
            ["abc界d".as_bytes()].concat()
        );
    }

    #[test]
    fn arbitrary_two_byte_inputs_never_panic() {
        for first in 0_u8..=u8::MAX {
            for second in 0_u8..=u8::MAX {
                let mut filter = RestrictedTerminalFilter::new();
                let _ = filter.push(&[first]);
                let _ = filter.push(&[second]);
                let _ = filter.finish();
            }
        }
    }
}
