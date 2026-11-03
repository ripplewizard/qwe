use clap::Parser;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use qwe_protocol::{
    PROTOCOL_VERSION, ProtocolErrorBody, RequestEnvelope, ResponseEnvelope, read_frame,
    validate_version, verify_same_uid, write_frame,
};
use qwe_runtime::BoundedJournal;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const JOURNAL_LIMIT: usize = 16 * 1024 * 1024;
const ATTACH_TAIL_BYTES: usize = 1024 * 1024;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    socket: String,
}

struct Session {
    pid: u32,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    journal: Arc<Mutex<BoundedJournal>>,
    screen: Arc<Mutex<String>>,
    running: Arc<AtomicBool>,
}

#[derive(Default)]
struct Host {
    sessions: HashMap<String, Session>,
    next_epoch: u64,
    active_epoch: u64,
}

fn error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

impl Host {
    fn create(&mut self, session_id: &str, payload: &Value) -> Result<Value, ProtocolErrorBody> {
        if self.sessions.contains_key(session_id) {
            return Err(error("already_exists", "session already exists"));
        }
        let program = payload
            .get("program")
            .and_then(Value::as_str)
            .unwrap_or("sh");
        let arguments: Vec<String> = payload
            .get("args")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| self.internal(error.to_string()))?;
        let mut command = CommandBuilder::new(program);
        command.args(arguments);
        command.env("TERM", "xterm-256color");
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| self.internal(error.to_string()))?;
        let pid = child
            .process_id()
            .ok_or_else(|| self.internal("session process has no PID".to_string()))?;
        drop(pair.slave);
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| self.internal(error.to_string()))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| self.internal(error.to_string()))?;
        let master = pair.master;
        let journal = Arc::new(Mutex::new(
            BoundedJournal::new(JOURNAL_LIMIT).map_err(|error| self.internal(error.to_string()))?,
        ));
        let screen = Arc::new(Mutex::new(String::new()));
        let running = Arc::new(AtomicBool::new(true));

        let thread_journal = Arc::clone(&journal);
        let thread_screen = Arc::clone(&screen);
        let thread_running = Arc::clone(&running);
        thread::spawn(move || {
            let _master = master;
            let mut parser = vt100::Parser::new(
                vt100::Parser::DEFAULT_ROWS,
                vt100::Parser::DEFAULT_COLS,
                2000,
            );
            let mut buffer = [0_u8; 8192];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                thread_journal
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .append(&buffer[..count]);
                parser.process(&buffer[..count]);
                *thread_screen
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = parser.screen().contents();
            }
            let _ = child.wait();
            thread_running.store(false, Ordering::SeqCst);
        });

        let session = Session {
            pid,
            writer: Arc::new(Mutex::new(writer)),
            journal,
            screen,
            running,
        };
        let status = Self::status(session_id, &session);
        self.sessions.insert(session_id.to_string(), session);
        Ok(status)
    }

    fn status(session_id: &str, session: &Session) -> Value {
        let journal = session
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        json!({
            "session_id": session_id,
            "pid": session.pid,
            "running": session.running.load(Ordering::SeqCst),
            "truncated_from": journal.truncated_from(),
            "total_written": journal.total_written()
        })
    }

    fn snapshot(session_id: &str, session: &Session) -> Value {
        let journal = session
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let screen = session
            .screen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tail = journal.tail(ATTACH_TAIL_BYTES);
        json!({
            "session_id": session_id,
            "pid": session.pid,
            "running": session.running.load(Ordering::SeqCst),
            "screen": *screen,
            "journal_hex": hex_encode(&tail),
            "snapshot_bytes": tail.len(),
            "truncated_from": journal.truncated_from(),
            "total_written": journal.total_written()
        })
    }

    fn input(&mut self, session_id: &str, payload: &Value) -> Result<Value, ProtocolErrorBody> {
        let data = payload
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| error("invalid_request", "missing input data"))?;
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| error("not_found", "session not found"))?;
        session
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write_all(data.as_bytes())
            .map_err(|error| self.internal(error.to_string()))?;
        Ok(Self::status(session_id, session))
    }

    fn handle(
        &mut self,
        operation: &str,
        host_epoch: Option<u64>,
        session_id: &str,
        payload: &Value,
    ) -> Result<Value, ProtocolErrorBody> {
        if operation == "HOST_REGISTER" {
            self.next_epoch += 1;
            self.active_epoch = self.next_epoch;
            return Ok(json!({
                "epoch": self.active_epoch,
                "protocol": PROTOCOL_VERSION
            }));
        }
        if host_epoch != Some(self.active_epoch) {
            return Err(error(
                "stale_epoch",
                format!("expected host epoch {}", self.active_epoch),
            ));
        }
        match operation {
            "PING" => Ok(json!({"host": "ok", "protocol": PROTOCOL_VERSION})),
            "HOST_LIST" => Ok(json!({
                "epoch": self.active_epoch,
                "sessions": self.sessions.keys().collect::<Vec<_>>()
            })),
            "SESSION_CREATE" => self.create(session_id, payload),
            "SESSION_STATUS" => self
                .sessions
                .get(session_id)
                .map(|session| Self::status(session_id, session))
                .ok_or_else(|| error("not_found", "session not found")),
            "SESSION_ATTACH" => self
                .sessions
                .get(session_id)
                .map(|session| Self::snapshot(session_id, session))
                .ok_or_else(|| error("not_found", "session not found")),
            "SESSION_INPUT" => self.input(session_id, payload),
            "SESSION_DETACH" => self
                .sessions
                .get(session_id)
                .map(|session| Self::status(session_id, session))
                .ok_or_else(|| error("not_found", "session not found")),
            "SESSION_CLOSE" => {
                let session = self
                    .sessions
                    .remove(session_id)
                    .ok_or_else(|| error("not_found", "session not found"))?;
                unsafe {
                    libc::kill(-(session.pid as libc::pid_t), libc::SIGTERM);
                }
                Ok(json!({"session_id": session_id, "state": "closed"}))
            }
            _ => Err(error("unsupported_operation", operation)),
        }
    }

    fn internal(&self, message: String) -> ProtocolErrorBody {
        error("internal", message)
    }
}

fn session_id(request: &RequestEnvelope) -> String {
    request
        .payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn serve(mut stream: UnixStream, host: &mut Host) {
    if verify_same_uid(&stream).is_err() {
        return;
    }
    let result = (|| {
        let request: RequestEnvelope = read_frame(&mut stream)?;
        validate_version(request.version).map_err(|error| {
            qwe_protocol::IoFrameError::Decode(qwe_protocol::DecodeError::Framing(error))
        })?;
        let id = session_id(&request);
        let response = ResponseEnvelope {
            version: PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            result: host.handle(
                &request.operation,
                request.host_epoch,
                &id,
                &request.payload,
            ),
        };
        write_frame(&mut stream, &response)
    })();
    if let Err(failure) = result {
        let response = ResponseEnvelope {
            version: PROTOCOL_VERSION,
            request_id: "unknown".into(),
            result: Err(error("protocol_error", failure.to_string())),
        };
        let _ = write_frame(&mut stream, &response);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if UnixStream::connect(&args.socket).is_ok() {
        return Err(format!("qwe-host is already running at {}", args.socket).into());
    }
    let _ = fs::remove_file(&args.socket);
    let session_result = unsafe { libc::setsid() };
    if session_result < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
        return Err(std::io::Error::last_os_error().into());
    }
    let listener = UnixListener::bind(&args.socket)?;
    fs::set_permissions(&args.socket, fs::Permissions::from_mode(0o600))?;
    let mut host = Host::default();
    for stream in listener.incoming() {
        serve(stream?, &mut host);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_registration_fences_old_epoch() {
        let mut host = Host::default();
        let first = host.handle("HOST_REGISTER", None, "", &json!({})).unwrap()["epoch"]
            .as_u64()
            .unwrap();
        let second = host.handle("HOST_REGISTER", None, "", &json!({})).unwrap()["epoch"]
            .as_u64()
            .unwrap();
        assert!(second > first);
        assert_eq!(
            host.handle("HOST_LIST", Some(first), "", &json!({}))
                .unwrap_err()
                .code,
            "stale_epoch"
        );
        assert!(
            host.handle("HOST_LIST", Some(second), "", &json!({}))
                .is_ok()
        );
    }
}
