use crate::lease::LeaseManager;
use crate::runtime::json_escape;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

struct Session {
    pid: u32,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<Vec<u8>>>,
    running: Arc<AtomicBool>,
    attached: Arc<AtomicBool>,
}

pub struct SessionManager {
    sessions: HashMap<String, Session>,
    leases: Arc<Mutex<LeaseManager>>,
}

impl SessionManager {
    pub fn new(leases: Arc<Mutex<LeaseManager>>) -> Self {
        Self {
            sessions: HashMap::new(),
            leases,
        }
    }

    fn require_owner(&self, session_id: &str, client_id: &str) -> bool {
        self.leases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_owner(session_id, client_id)
    }

    fn response(request_id: &str, session_id: &str, session: &Session) -> String {
        let output = session
            .output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        format!(
            r#"{{"ok":true,"request_id":"{}","session_id":"{}","pid":{},"running":{},"attached":{},"output_bytes":{},"snapshot_hex":"{}"}}"#,
            json_escape(request_id),
            json_escape(session_id),
            session.pid,
            session.running.load(Ordering::SeqCst),
            session.attached.load(Ordering::SeqCst),
            output.len(),
            encode_hex(&output)
        )
    }

    fn error(request_id: &str, code: &str, message: &str) -> String {
        format!(
            r#"{{"ok":false,"request_id":"{}","error":"{}","message":"{}"}}"#,
            json_escape(request_id),
            code,
            json_escape(message)
        )
    }

    pub fn start(
        &mut self,
        request_id: &str,
        session_id: &str,
        log_path: &str,
        program: &str,
        arguments: &[String],
    ) -> String {
        if self.sessions.contains_key(session_id) {
            return Self::error(
                request_id,
                "already_exists",
                &format!("session already exists: {session_id}"),
            );
        }
        let output_file = match OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(log_path)
        {
            Ok(file) => file,
            Err(error) => return Self::error(request_id, "internal", &error.to_string()),
        };
        let pty_system = native_pty_system();
        let pair = match pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => pair,
            Err(error) => return Self::error(request_id, "internal", &error.to_string()),
        };
        let mut command = CommandBuilder::new(program);
        for argument in arguments {
            command.arg(argument);
        }
        let mut child = match pair.slave.spawn_command(command) {
            Ok(child) => child,
            Err(error) => return Self::error(request_id, "internal", &error.to_string()),
        };
        drop(pair.slave);
        let pid = match child.process_id() {
            Some(pid) => pid,
            None => {
                return Self::error(
                    request_id,
                    "internal",
                    "session process does not expose a process id",
                )
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => return Self::error(request_id, "internal", &error.to_string()),
        };
        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => return Self::error(request_id, "internal", &error.to_string()),
        };
        let master = pair.master;
        let output = Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(AtomicBool::new(true));
        let attached = Arc::new(AtomicBool::new(false));
        let thread_output = Arc::clone(&output);
        let thread_running = Arc::clone(&running);
        let thread_attached = Arc::clone(&attached);
        let leases = Arc::clone(&self.leases);
        let thread_session_id = session_id.to_string();

        self.sessions.insert(
            session_id.to_string(),
            Session {
                pid,
                writer,
                output,
                running,
                attached,
            },
        );
        let session = self.sessions.get(session_id).expect("session was inserted");
        let response = Self::response(request_id, session_id, session);
        thread::spawn(move || {
            let _master = master;
            let mut output_file = output_file;
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let _ = output_file.write_all(&buffer[..count]);
                        thread_output
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .extend_from_slice(&buffer[..count]);
                        let mut leases = leases
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        leases.record_output(&thread_session_id, count as i64);
                    }
                    Err(_) => break,
                }
            }
            let _ = child.wait();
            thread_running.store(false, Ordering::SeqCst);
            thread_attached.store(false, Ordering::SeqCst);
        });
        response
    }

    pub fn attach(&mut self, request_id: &str, session_id: &str, client_id: &str) -> String {
        if !self.require_owner(session_id, client_id) {
            return Self::error(
                request_id,
                "not_owner",
                &format!("client does not own session {session_id}"),
            );
        }
        let Some(session) = self.sessions.get_mut(session_id) else {
            return Self::error(
                request_id,
                "not_found",
                &format!("session not found: {session_id}"),
            );
        };
        session.attached.store(true, Ordering::SeqCst);
        Self::response(request_id, session_id, session)
    }

    pub fn detach(&mut self, request_id: &str, session_id: &str, client_id: &str) -> String {
        if !self.require_owner(session_id, client_id) {
            return Self::error(
                request_id,
                "not_owner",
                &format!("client does not own session {session_id}"),
            );
        }
        let Some(session) = self.sessions.get_mut(session_id) else {
            return Self::error(
                request_id,
                "not_found",
                &format!("session not found: {session_id}"),
            );
        };
        session.attached.store(false, Ordering::SeqCst);
        Self::response(request_id, session_id, session)
    }

    pub fn input(
        &mut self,
        request_id: &str,
        session_id: &str,
        client_id: &str,
        encoded: &str,
    ) -> String {
        let data = match decode_hex(encoded) {
            Ok(data) => data,
            Err(error) => return Self::error(request_id, "invalid_request", &error),
        };
        let authorized = self
            .leases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .authorize_input(session_id, client_id);
        if !authorized {
            return Self::error(
                request_id,
                "not_owner",
                &format!("client does not own session {session_id}"),
            );
        }
        let Some(session) = self.sessions.get_mut(session_id) else {
            return Self::error(
                request_id,
                "not_found",
                &format!("session not found: {session_id}"),
            );
        };
        if !session.running.load(Ordering::SeqCst) {
            return Self::error(
                request_id,
                "not_running",
                &format!("session is not running: {session_id}"),
            );
        }
        if !session.attached.load(Ordering::SeqCst) {
            return Self::error(
                request_id,
                "not_attached",
                &format!("session is detached: {session_id}"),
            );
        }
        if let Err(error) = session.write_all(&data) {
            return Self::error(request_id, "internal", &error);
        }
        Self::response(request_id, session_id, session)
    }

    pub fn status(&self, request_id: &str, session_id: &str) -> String {
        let Some(session) = self.sessions.get(session_id) else {
            return Self::error(
                request_id,
                "not_found",
                &format!("session not found: {session_id}"),
            );
        };
        Self::response(request_id, session_id, session)
    }
}

impl Session {
    fn write_all(&mut self, data: &[u8]) -> Result<(), String> {
        self.writer
            .write_all(data)
            .map_err(|error| error.to_string())?;
        self.writer.flush().map_err(|error| error.to_string())
    }
}

fn encode_hex(input: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(input.len() * 2);
    for byte in input {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_hex(input: &str) -> Result<Vec<u8>, String> {
    if !input.len().is_multiple_of(2) {
        return Err("hex input must contain an even number of characters".to_string());
    }
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        decoded.push((high << 4) | low);
    }
    Ok(decoded)
}

fn hex_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hexadecimal input".to_string()),
    }
}
