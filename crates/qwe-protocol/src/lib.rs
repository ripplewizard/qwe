use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use thiserror::Error;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const ATTACH_PROTOCOL_VERSION: u16 = 1;
pub const MAX_ATTACH_FRAME_PAYLOAD_BYTES: usize = 1024 * 1024;
pub const MAX_ATTACH_OUTPUT_BYTES: usize = 64 * 1024;

pub mod attach {
    use serde::{Deserialize, Serialize};
    use thiserror::Error;

    use super::{ATTACH_PROTOCOL_VERSION, MAX_ATTACH_FRAME_PAYLOAD_BYTES, MAX_ATTACH_OUTPUT_BYTES};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Direction {
        ClientToServer,
        ServerToClient,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum FrameType {
        AttachOpen = 0x01,
        Input = 0x02,
        Resize = 0x03,
        Ack = 0x04,
        Detach = 0x05,
        AttachAccepted = 0x81,
        Snapshot = 0x82,
        Output = 0x83,
        Resized = 0x84,
        ProcessExited = 0x85,
        OwnershipRevoked = 0x86,
        Error = 0x87,
    }

    impl FrameType {
        pub const ALL: [Self; 12] = [
            Self::AttachOpen,
            Self::Input,
            Self::Resize,
            Self::Ack,
            Self::Detach,
            Self::AttachAccepted,
            Self::Snapshot,
            Self::Output,
            Self::Resized,
            Self::ProcessExited,
            Self::OwnershipRevoked,
            Self::Error,
        ];

        #[must_use]
        pub fn direction(self) -> Direction {
            match self {
                Self::AttachOpen | Self::Input | Self::Resize | Self::Ack | Self::Detach => {
                    Direction::ClientToServer
                }
                Self::AttachAccepted
                | Self::Snapshot
                | Self::Output
                | Self::Resized
                | Self::ProcessExited
                | Self::OwnershipRevoked
                | Self::Error => Direction::ServerToClient,
            }
        }

        #[must_use]
        pub fn code(self) -> u8 {
            self as u8
        }

        #[must_use]
        pub fn name(self) -> &'static str {
            match self {
                Self::AttachOpen => "ATTACH_OPEN",
                Self::Input => "INPUT",
                Self::Resize => "RESIZE",
                Self::Ack => "ACK",
                Self::Detach => "DETACH",
                Self::AttachAccepted => "ATTACH_ACCEPTED",
                Self::Snapshot => "SNAPSHOT",
                Self::Output => "OUTPUT",
                Self::Resized => "RESIZED",
                Self::ProcessExited => "PROCESS_EXITED",
                Self::OwnershipRevoked => "OWNERSHIP_REVOKED",
                Self::Error => "ERROR",
            }
        }
    }

    impl TryFrom<u8> for FrameType {
        type Error = AttachCodecError;

        fn try_from(value: u8) -> Result<Self, AttachCodecError> {
            Self::ALL
                .into_iter()
                .find(|frame_type| frame_type.code() == value)
                .ok_or(AttachCodecError::UnknownFrameType { code: value })
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameHeader {
        pub frame_type: FrameType,
        pub payload_length: usize,
    }

    impl FrameHeader {
        pub fn encode(self) -> Result<[u8; 5], AttachCodecError> {
            self.validate()?;
            let length = u32::try_from(self.payload_length).map_err(|_| {
                AttachCodecError::PayloadTooLarge {
                    frame_type: self.frame_type,
                    size: self.payload_length,
                    maximum: MAX_ATTACH_FRAME_PAYLOAD_BYTES,
                }
            })?;
            let mut encoded = [0_u8; 5];
            encoded[0] = self.frame_type.code();
            encoded[1..].copy_from_slice(&length.to_be_bytes());
            Ok(encoded)
        }

        pub fn decode(bytes: [u8; 5]) -> Result<Self, AttachCodecError> {
            let header = Self {
                frame_type: FrameType::try_from(bytes[0])?,
                payload_length: u32::from_be_bytes(bytes[1..].try_into().expect("fixed size"))
                    as usize,
            };
            header.validate()?;
            Ok(header)
        }

        fn validate(self) -> Result<(), AttachCodecError> {
            let maximum = if self.frame_type == FrameType::Output {
                MAX_ATTACH_OUTPUT_BYTES
            } else {
                MAX_ATTACH_FRAME_PAYLOAD_BYTES
            };
            if self.payload_length > maximum {
                return Err(AttachCodecError::PayloadTooLarge {
                    frame_type: self.frame_type,
                    size: self.payload_length,
                    maximum,
                });
            }
            Ok(())
        }
    }

    #[derive(Debug, Error, PartialEq, Eq)]
    pub enum AttachCodecError {
        #[error("unknown attach frame type 0x{code:02x}")]
        UnknownFrameType { code: u8 },
        #[error(
            "{frame_type:?} payload is {size} bytes; maximum for this frame is {maximum} bytes"
        )]
        PayloadTooLarge {
            frame_type: FrameType,
            size: usize,
            maximum: usize,
        },
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum AttachMode {
        #[default]
        Exclusive,
        ReadOnly,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum MouseProtocolMode {
        None,
        Press,
        PressRelease,
        ButtonMotion,
        AnyMotion,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum MouseProtocolEncoding {
        Default,
        Utf8,
        Sgr,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum CursorStyle {
        Default,
        BlinkBlock,
        SteadyBlock,
        BlinkUnderline,
        SteadyUnderline,
        BlinkVerticalBar,
        SteadyVerticalBar,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct AttachOpen {
        pub protocol_version: u16,
        pub session_id: String,
        pub owner: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub reclaim_token: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub lease_generation: Option<u64>,
        pub rows: u16,
        pub columns: u16,
        #[serde(default)]
        pub mode: AttachMode,
    }

    impl AttachOpen {
        #[must_use]
        pub fn versioned(
            session_id: impl Into<String>,
            owner: impl Into<String>,
            rows: u16,
            columns: u16,
        ) -> Self {
            Self {
                protocol_version: ATTACH_PROTOCOL_VERSION,
                session_id: session_id.into(),
                owner: owner.into(),
                reclaim_token: None,
                lease_generation: None,
                rows,
                columns,
                mode: AttachMode::default(),
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub version: u16,
    pub request_id: String,
    pub idempotency_key: Option<String>,
    pub host_epoch: Option<u64>,
    pub operation: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub version: u16,
    pub request_id: String,
    pub result: Result<serde_json::Value, ProtocolErrorBody>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FramingError {
    #[error("protocol version {received} is incompatible with {expected}")]
    IncompatibleVersion { received: u16, expected: u16 },
    #[error("frame is too large: {size} bytes")]
    FrameTooLarge { size: usize },
    #[error("truncated frame")]
    Truncated,
}

pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let body = serde_json::to_vec(value)?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn decode_body<T: for<'de> Deserialize<'de>>(
    length: u32,
    body: &[u8],
) -> Result<T, DecodeError> {
    let length = length as usize;
    if length > MAX_FRAME_BYTES {
        return Err(DecodeError::Framing(FramingError::FrameTooLarge {
            size: length,
        }));
    }
    if body.len() != length {
        return Err(DecodeError::Framing(FramingError::Truncated));
    }
    serde_json::from_slice(body).map_err(DecodeError::Json)
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error(transparent)]
    Framing(#[from] FramingError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum IoFrameError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum PeerError {
    #[error("peer credential lookup failed: {0}")]
    Credential(String),
    #[error("peer user {peer_uid} does not match current user {current_uid}")]
    Unauthorized { peer_uid: u32, current_uid: u32 },
}

#[cfg(target_os = "linux")]
fn socket_peer_uid(stream: &UnixStream) -> Result<u32, PeerError> {
    let mut credential = std::mem::MaybeUninit::<libc::ucred>::zeroed();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credential.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(PeerError::Credential(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(unsafe { credential.assume_init() }.uid)
}

#[cfg(target_os = "macos")]
fn socket_peer_uid(stream: &UnixStream) -> Result<u32, PeerError> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result != 0 {
        return Err(PeerError::Credential(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(uid)
}

pub fn verify_same_uid(stream: &UnixStream) -> Result<(), PeerError> {
    let peer_uid = socket_peer_uid(stream)?;
    let current_uid = unsafe { libc::geteuid() };
    if peer_uid == current_uid {
        Ok(())
    } else {
        Err(PeerError::Unauthorized {
            peer_uid,
            current_uid,
        })
    }
}

pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<(), IoFrameError> {
    writer.write_all(&encode_frame(value)?)?;
    writer.flush()?;
    Ok(())
}

pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
) -> Result<T, IoFrameError> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length);
    if length as usize > MAX_FRAME_BYTES {
        return Err(DecodeError::Framing(FramingError::FrameTooLarge {
            size: length as usize,
        })
        .into());
    }
    let mut body = vec![0_u8; length as usize];
    reader.read_exact(&mut body)?;
    Ok(decode_body(length, &body)?)
}

pub fn validate_version(version: u16) -> Result<(), FramingError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(FramingError::IncompatibleVersion {
            received: version,
            expected: PROTOCOL_VERSION,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let request = RequestEnvelope {
            version: PROTOCOL_VERSION,
            request_id: "req-1".into(),
            idempotency_key: Some("key-1".into()),
            host_epoch: Some(1),
            operation: "task.start".into(),
            payload: serde_json::json!({"name": "demo"}),
        };
        let frame = encode_frame(&request).unwrap();
        let length = u32::from_be_bytes(frame[..4].try_into().unwrap());
        let decoded: RequestEnvelope = decode_body(length, &frame[4..]).unwrap();
        assert_eq!(decoded.request_id, "req-1");
    }

    #[test]
    fn rejects_incompatible_version() {
        assert!(matches!(
            validate_version(99),
            Err(FramingError::IncompatibleVersion { .. })
        ));
    }

    #[test]
    fn accepts_same_uid_socket_peer() {
        let (left, right) = UnixStream::pair().unwrap();
        verify_same_uid(&left).unwrap();
        verify_same_uid(&right).unwrap();
    }

    #[test]
    fn attach_frame_header_round_trips() {
        let header = attach::FrameHeader {
            frame_type: attach::FrameType::AttachOpen,
            payload_length: 42,
        };
        assert_eq!(
            attach::FrameHeader::decode(header.encode().unwrap()).unwrap(),
            header
        );
    }

    #[test]
    fn attach_codec_rejects_unknown_and_oversized_frames() {
        assert_eq!(
            attach::FrameType::try_from(0x42),
            Err(attach::AttachCodecError::UnknownFrameType { code: 0x42 })
        );
        let output = attach::FrameHeader {
            frame_type: attach::FrameType::Output,
            payload_length: MAX_ATTACH_OUTPUT_BYTES + 1,
        };
        assert!(matches!(
            output.encode(),
            Err(attach::AttachCodecError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn attach_open_defaults_to_exclusive_mode() {
        let value = serde_json::to_value(attach::AttachOpen::versioned("s", "o", 24, 80)).unwrap();
        assert_eq!(value["mode"], "exclusive");
        assert!(value.get("reclaim_token").is_none());
    }

    #[test]
    fn attach_frame_contract_matches_canonical_schema() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../protocol/schema/qwe-v1.schema.json"))
                .unwrap();
        for frame_type in attach::FrameType::ALL {
            let direction = match frame_type.direction() {
                attach::Direction::ClientToServer => "client_to_server",
                attach::Direction::ServerToClient => "server_to_client",
            };
            assert_eq!(
                schema["x-qwe-frame-types"][direction][frame_type.name()],
                frame_type.code()
            );
        }
        assert_eq!(
            schema["x-qwe-frame-header"]["maximum_payload_bytes"],
            MAX_ATTACH_FRAME_PAYLOAD_BYTES
        );
        assert_eq!(
            schema["$defs"]["OutputMetadata"]["properties"]["maximum_output_bytes"]["const"],
            MAX_ATTACH_OUTPUT_BYTES
        );
    }
}
