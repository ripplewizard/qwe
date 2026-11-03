use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use thiserror::Error;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

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
}
