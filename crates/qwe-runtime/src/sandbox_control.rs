pub const CONTROL_VERSION: u8 = 1;
pub const CONTROL_KIND_SIGNAL: u8 = 1;
pub const CONTROL_HEADER_BYTES: usize = 4;
pub const CONTROL_MAX_PAYLOAD: usize = 8;
pub const CONTROL_MAX_BUFFER: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneControlMessage {
    Signal(libc::c_int),
}

#[derive(Debug, Default)]
pub struct PaneControlDecoder {
    buffered: Vec<u8>,
}

impl PaneControlDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<PaneControlMessage>, String> {
        if self.buffered.len().saturating_add(bytes.len()) > CONTROL_MAX_BUFFER {
            return Err("control message buffer exceeded 1024 bytes".into());
        }
        self.buffered.extend_from_slice(bytes);
        let mut messages = Vec::new();
        loop {
            if self.buffered.len() < CONTROL_HEADER_BYTES {
                return Ok(messages);
            }
            let version = self.buffered[0];
            let kind = self.buffered[1];
            let payload_length =
                usize::from(u16::from_be_bytes([self.buffered[2], self.buffered[3]]));
            if version != CONTROL_VERSION {
                return Err(format!("unsupported control protocol version {version}"));
            }
            if payload_length > CONTROL_MAX_PAYLOAD {
                return Err(format!(
                    "control payload is {payload_length} bytes; maximum is {CONTROL_MAX_PAYLOAD}"
                ));
            }
            let frame_length = CONTROL_HEADER_BYTES + payload_length;
            if self.buffered.len() < frame_length {
                return Ok(messages);
            }
            let payload = self.buffered[CONTROL_HEADER_BYTES..frame_length].to_vec();
            self.buffered.drain(..frame_length);
            messages.push(decode_control_message(kind, &payload)?);
        }
    }

    pub fn finish(&mut self) {
        self.buffered.clear();
    }
}

fn decode_control_message(kind: u8, payload: &[u8]) -> Result<PaneControlMessage, String> {
    if kind != CONTROL_KIND_SIGNAL {
        return Err(format!("unknown control message kind {kind}"));
    }
    let [signal] = payload else {
        return Err("signal control message must contain exactly one byte".into());
    };
    let signal = libc::c_int::from(*signal);
    if !matches!(
        signal,
        libc::SIGINT | libc::SIGTERM | libc::SIGHUP | libc::SIGQUIT
    ) {
        return Err(format!("control signal {signal} is not permitted"));
    }
    Ok(PaneControlMessage::Signal(signal))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(version: u8, kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![version, kind];
        frame.extend(u16::try_from(payload.len()).unwrap().to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn accepts_fragmented_and_coalesced_signal_frames() {
        let mut decoder = PaneControlDecoder::default();
        let first = frame(CONTROL_VERSION, CONTROL_KIND_SIGNAL, &[libc::SIGTERM as u8]);
        let second = frame(CONTROL_VERSION, CONTROL_KIND_SIGNAL, &[libc::SIGHUP as u8]);
        assert!(decoder.push(&first[..2]).unwrap().is_empty());
        assert_eq!(
            decoder.push(&first[2..]).unwrap(),
            vec![PaneControlMessage::Signal(libc::SIGTERM)]
        );
        assert_eq!(
            decoder.push(&second).unwrap(),
            vec![PaneControlMessage::Signal(libc::SIGHUP)]
        );
    }

    #[test]
    fn rejects_unknown_unsafe_and_oversized_frames() {
        let mut decoder = PaneControlDecoder::default();
        assert!(
            decoder
                .push(&frame(CONTROL_VERSION + 1, CONTROL_KIND_SIGNAL, &[1]))
                .is_err()
        );
        let mut decoder = PaneControlDecoder::default();
        assert!(
            decoder
                .push(&frame(CONTROL_VERSION, CONTROL_KIND_SIGNAL + 1, &[1]))
                .is_err()
        );
        let mut decoder = PaneControlDecoder::default();
        assert!(
            decoder
                .push(&frame(
                    CONTROL_VERSION,
                    CONTROL_KIND_SIGNAL,
                    &[libc::SIGKILL as u8],
                ))
                .is_err()
        );
        let mut decoder = PaneControlDecoder::default();
        assert!(
            decoder
                .push(&[CONTROL_VERSION, CONTROL_KIND_SIGNAL, 0, 9])
                .is_err()
        );
        let mut decoder = PaneControlDecoder::default();
        assert!(decoder.push(&vec![0_u8; CONTROL_MAX_BUFFER + 1]).is_err());
    }

    #[test]
    fn arbitrary_inputs_never_panic() {
        for first in 0_u8..=u8::MAX {
            let mut decoder = PaneControlDecoder::default();
            let _ = decoder.push(&[first]);
            for second in 0_u8..=u8::MAX {
                let _ = decoder.push(&[second]);
                decoder.finish();
            }
        }
    }
}
