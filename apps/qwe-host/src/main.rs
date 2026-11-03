use clap::Parser;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use qwe_core::ProcessIdentity;
use qwe_protocol::{
    PROTOCOL_VERSION, ProtocolErrorBody, RequestEnvelope, ResponseEnvelope, read_frame,
    validate_version, verify_same_uid, write_frame,
};
use qwe_runtime::{BoundedJournal, process_identity};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const JOURNAL_LIMIT: usize = 16 * 1024 * 1024;
const ATTACH_TAIL_BYTES: usize = 1024 * 1024;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    socket: Option<String>,
    #[arg(long)]
    runtime_dir: Option<PathBuf>,
    #[arg(long, hide = true)]
    worker_socket: Option<PathBuf>,
    #[arg(long, hide = true)]
    worker_session_id: Option<String>,
    #[arg(long, hide = true, default_value = "sh")]
    worker_program: String,
    #[arg(long = "worker-arg", hide = true)]
    worker_args: Vec<String>,
}

struct WorkerSession {
    pid: u32,
    identity: ProcessIdentity,
    writer: Mutex<Box<dyn Write + Send>>,
    journal: Mutex<BoundedJournal>,
    screen: Mutex<String>,
    running: AtomicBool,
    exit_code: Mutex<Option<i32>>,
    exit_signal: Mutex<Option<String>>,
}

struct WorkerHandle {
    socket: PathBuf,
    worker_pid: u32,
}

struct Host {
    sessions: HashMap<String, WorkerHandle>,
    runtime_dir: PathBuf,
    epoch_file: PathBuf,
    next_epoch: u64,
    active_epoch: u64,
}

enum HostAction {
    Done(Result<Value, ProtocolErrorBody>),
    Forward {
        socket: PathBuf,
        worker_pid: u32,
        operation: String,
        payload: Value,
        remove_session: Option<String>,
    },
}

fn error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
    }
}

fn request_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

fn session_id(request: &RequestEnvelope) -> String {
    request
        .payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn worker_socket(runtime_dir: &Path, session_id: &str) -> PathBuf {
    runtime_dir.join(format!("{}.sock", hex_encode(session_id.as_bytes())))
}

fn call_socket(socket: &Path, operation: &str, payload: Value) -> Result<Value, ProtocolErrorBody> {
    let mut stream = UnixStream::connect(socket).map_err(|failure| {
        error(
            "worker_unavailable",
            format!("{}: {failure}", socket.display()),
        )
    })?;
    verify_same_uid(&stream).map_err(|failure| error("unauthorized_peer", failure.to_string()))?;
    let request = RequestEnvelope {
        version: PROTOCOL_VERSION,
        request_id: request_id("qwe-host"),
        idempotency_key: None,
        host_epoch: None,
        operation: operation.into(),
        payload,
    };
    write_frame(&mut stream, &request)
        .map_err(|failure| error("transport", failure.to_string()))?;
    let response: ResponseEnvelope =
        read_frame(&mut stream).map_err(|failure| error("transport", failure.to_string()))?;
    response.result
}

impl Host {
    fn recover(runtime_dir: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        fs::create_dir_all(&runtime_dir)?;
        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700))?;
        let epoch_file = runtime_dir.join("epoch");
        let next_epoch = match fs::read_to_string(&epoch_file) {
            Ok(value) => value
                .trim()
                .parse::<u64>()
                .map_err(|failure| format!("invalid host epoch file: {failure}"))?,
            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => 0,
            Err(failure) => return Err(Box::new(failure)),
        };
        let mut sessions = HashMap::new();
        for entry in fs::read_dir(&runtime_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("sock") {
                continue;
            }
            match call_socket(&path, "WORKER_HELLO", json!({})) {
                Ok(value) => {
                    if let (Some(id), Some(worker_pid)) = (
                        value.get("session_id").and_then(Value::as_str),
                        value.get("worker_pid").and_then(Value::as_u64),
                    ) {
                        sessions.insert(
                            id.to_string(),
                            WorkerHandle {
                                socket: path.clone(),
                                worker_pid: worker_pid as u32,
                            },
                        );
                    }
                }
                Err(_) => {
                    let _ = fs::remove_file(path);
                }
            }
        }
        Ok(Self {
            sessions,
            runtime_dir,
            epoch_file,
            next_epoch,
            active_epoch: 0,
        })
    }

    fn persist_epoch(&self, epoch: u64) -> Result<(), ProtocolErrorBody> {
        let temporary = self.epoch_file.with_extension("tmp");
        fs::write(&temporary, format!("{epoch}\n"))
            .and_then(|()| fs::File::open(&temporary)?.sync_all())
            .and_then(|()| fs::rename(&temporary, &self.epoch_file))
            .map_err(|failure| error("persistence_failure", failure.to_string()))
    }

    fn create(&mut self, session_id: &str, payload: &Value) -> Result<Value, ProtocolErrorBody> {
        if self.sessions.contains_key(session_id) {
            return Err(error("already_exists", "session already exists"));
        }
        let socket = worker_socket(&self.runtime_dir, session_id);
        let _ = fs::remove_file(&socket);
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
        let executable =
            std::env::current_exe().map_err(|failure| error("internal", failure.to_string()))?;
        let mut command = Command::new(executable);
        command
            .arg("--worker-socket")
            .arg(&socket)
            .arg("--worker-session-id")
            .arg(session_id)
            .arg("--worker-program")
            .arg(program)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for argument in arguments {
            command.arg("--worker-arg").arg(argument);
        }
        let mut worker = command
            .spawn()
            .map_err(|failure| error("internal", failure.to_string()))?;
        let result = (|| {
            for _ in 0..200 {
                if let Ok(value) = call_socket(&socket, "WORKER_HELLO", json!({})) {
                    let status =
                        call_socket(&socket, "SESSION_STATUS", json!({"session_id": session_id}))?;
                    let worker_pid =
                        value
                            .get("worker_pid")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| {
                                error("worker_start_failed", "worker hello has no process ID")
                            })?;
                    self.sessions.insert(
                        session_id.to_string(),
                        WorkerHandle {
                            socket: socket.clone(),
                            worker_pid: worker_pid as u32,
                        },
                    );
                    return Ok(status);
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error(
                "worker_start_failed",
                "session worker did not become ready",
            ))
        })();
        if result.is_err() {
            let _ = worker.kill();
            let _ = worker.wait();
            let _ = fs::remove_file(socket);
        }
        result
    }

    fn list(&mut self) -> Result<Value, ProtocolErrorBody> {
        let mut sessions = Vec::new();
        let mut stale = Vec::new();
        for (id, handle) in &self.sessions {
            match call_socket(&handle.socket, "SESSION_STATUS", json!({"session_id": id})) {
                Ok(status) => sessions.push(status),
                Err(_) => stale.push(id.clone()),
            }
        }
        for id in stale {
            self.sessions.remove(&id);
        }
        Ok(json!({
            "epoch": self.active_epoch,
            "sessions": sessions
        }))
    }

    fn dispatch(&mut self, request: &RequestEnvelope) -> HostAction {
        if request.operation == "HOST_REGISTER" {
            let next_epoch = self.next_epoch + 1;
            if let Err(failure) = self.persist_epoch(next_epoch) {
                return HostAction::Done(Err(failure));
            }
            self.next_epoch = next_epoch;
            self.active_epoch = next_epoch;
            return HostAction::Done(Ok(json!({
                "epoch": self.active_epoch,
                "protocol": PROTOCOL_VERSION,
                "recovered_sessions": self.sessions.len()
            })));
        }
        if request.host_epoch != Some(self.active_epoch) {
            return HostAction::Done(Err(error(
                "stale_epoch",
                format!("expected host epoch {}", self.active_epoch),
            )));
        }
        match request.operation.as_str() {
            "PING" => HostAction::Done(Ok(json!({
                "host": "ok",
                "protocol": PROTOCOL_VERSION
            }))),
            "HOST_LIST" => HostAction::Done(self.list()),
            "SESSION_CREATE" => {
                let id = session_id(request);
                HostAction::Done(self.create(&id, &request.payload))
            }
            "SESSION_STATUS" | "SESSION_ATTACH" | "SESSION_INPUT" | "SESSION_DETACH"
            | "SESSION_CLOSE" => {
                let id = session_id(request);
                let Some(handle) = self.sessions.get(&id) else {
                    return HostAction::Done(Err(error(
                        "terminal_lost",
                        "session is not owned by a live worker",
                    )));
                };
                HostAction::Forward {
                    socket: handle.socket.clone(),
                    worker_pid: handle.worker_pid,
                    operation: request.operation.clone(),
                    payload: request.payload.clone(),
                    remove_session: (request.operation == "SESSION_CLOSE").then_some(id),
                }
            }
            _ => HostAction::Done(Err(error("unsupported_operation", &request.operation))),
        }
    }
}

impl WorkerSession {
    fn create(
        program: &str,
        arguments: &[String],
    ) -> Result<Arc<Self>, Box<dyn std::error::Error>> {
        let pair = native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut command = CommandBuilder::new(program);
        command.args(arguments.iter());
        command.env("TERM", "xterm-256color");
        let mut child = pair.slave.spawn_command(command)?;
        let pid = child
            .process_id()
            .ok_or("session process has no PID".to_string())?;
        let identity = process_identity(pid)?;
        drop(pair.slave);
        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        let master = pair.master;
        let session = Arc::new(Self {
            pid,
            identity,
            writer: Mutex::new(writer),
            journal: Mutex::new(BoundedJournal::new(JOURNAL_LIMIT)?),
            screen: Mutex::new(String::new()),
            running: AtomicBool::new(true),
            exit_code: Mutex::new(None),
            exit_signal: Mutex::new(None),
        });
        let reader_session = Arc::clone(&session);
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
                reader_session
                    .journal
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .append(&buffer[..count]);
                parser.process(&buffer[..count]);
                *reader_session
                    .screen
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = parser.screen().contents();
            }
            if let Ok(status) = child.wait() {
                *reader_session
                    .exit_code
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(status.exit_code() as i32);
                *reader_session
                    .exit_signal
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    status.signal().map(str::to_string);
            }
            reader_session.running.store(false, Ordering::SeqCst);
        });
        Ok(session)
    }

    fn status(&self, session_id: &str) -> Value {
        let journal = self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let exit_code = *self
            .exit_code
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let exit_signal = self
            .exit_signal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        json!({
            "session_id": session_id,
            "worker_pid": std::process::id(),
            "pid": self.pid,
            "pgid": self.identity.pgid,
            "start_token": self.identity.start_token,
            "process_identity": self.identity,
            "running": self.running.load(Ordering::SeqCst),
            "state": if self.running.load(Ordering::SeqCst) { "running" } else { "exited" },
            "exit_code": exit_code,
            "exit_signal": exit_signal,
            "truncated_from": journal.truncated_from(),
            "total_written": journal.total_written()
        })
    }

    fn snapshot(&self, session_id: &str) -> Value {
        let running = self.running.load(Ordering::SeqCst);
        let journal = self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let screen = self
            .screen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let exit_code = *self
            .exit_code
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let exit_signal = self
            .exit_signal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let tail = journal.tail(ATTACH_TAIL_BYTES);
        json!({
            "session_id": session_id,
            "worker_pid": std::process::id(),
            "pid": self.pid,
            "pgid": self.identity.pgid,
            "start_token": self.identity.start_token,
            "process_identity": self.identity,
            "running": running,
            "state": if running { "running" } else { "exited" },
            "exit_code": exit_code,
            "exit_signal": exit_signal,
            "screen": *screen,
            "journal_hex": hex_encode(&tail),
            "snapshot_bytes": tail.len(),
            "truncated_from": journal.truncated_from(),
            "total_written": journal.total_written()
        })
    }

    fn input(&self, session_id: &str, payload: &Value) -> Result<Value, ProtocolErrorBody> {
        let data = payload
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| error("invalid_request", "missing input data"))?;
        self.writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write_all(data.as_bytes())
            .map_err(|failure| error("internal", failure.to_string()))?;
        Ok(self.status(session_id))
    }

    fn close(&self, session_id: &str) -> Value {
        unsafe {
            libc::kill(-(self.pid as libc::pid_t), libc::SIGTERM);
        }
        json!({"session_id": session_id, "state": "closed"})
    }
}

fn worker_handle(
    session: &Arc<WorkerSession>,
    operation: &str,
    session_id: &str,
    payload: &Value,
) -> Result<Value, ProtocolErrorBody> {
    match operation {
        "WORKER_HELLO" | "SESSION_STATUS" | "SESSION_DETACH" => Ok(session.status(session_id)),
        "SESSION_ATTACH" => Ok(session.snapshot(session_id)),
        "SESSION_INPUT" => session.input(session_id, payload),
        "SESSION_CLOSE" => Ok(session.close(session_id)),
        _ => Err(error("unsupported_operation", operation)),
    }
}

fn serve_worker(mut stream: UnixStream, session: &Arc<WorkerSession>, session_id: &str) {
    if verify_same_uid(&stream).is_err() {
        return;
    }
    let result = (|| {
        let request: RequestEnvelope = read_frame(&mut stream)?;
        validate_version(request.version).map_err(|failure| {
            qwe_protocol::IoFrameError::Decode(qwe_protocol::DecodeError::Framing(failure))
        })?;
        let response = ResponseEnvelope {
            version: PROTOCOL_VERSION,
            request_id: request.request_id,
            result: worker_handle(session, &request.operation, session_id, &request.payload),
        };
        write_frame(&mut stream, &response)
    })();
    if let Err(failure) = result {
        eprintln!("worker protocol error: {failure}");
    }
}

fn run_worker(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let socket = args.worker_socket.ok_or("missing --worker-socket")?;
    let session_id = args
        .worker_session_id
        .ok_or("missing --worker-session-id")?;
    if UnixStream::connect(&socket).is_ok() {
        return Err(format!("session worker already running at {}", socket.display()).into());
    }
    let _ = fs::remove_file(&socket);
    let session_result = unsafe { libc::setsid() };
    if session_result < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
        return Err(std::io::Error::last_os_error().into());
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let session = WorkerSession::create(&args.worker_program, &args.worker_args)?;
    for stream in listener.incoming() {
        let session = Arc::clone(&session);
        let session_id = session_id.clone();
        thread::spawn(move || serve_worker(stream.unwrap(), &session, &session_id));
    }
    Ok(())
}

fn serve_host(
    mut stream: UnixStream,
    host: &Arc<Mutex<Host>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if verify_same_uid(&stream).is_err() {
        return Ok(());
    }
    let request: RequestEnvelope = read_frame(&mut stream)?;
    validate_version(request.version).map_err(|failure| {
        qwe_protocol::IoFrameError::Decode(qwe_protocol::DecodeError::Framing(failure))
    })?;
    let action = host
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .dispatch(&request);
    let result = match action {
        HostAction::Done(result) => result,
        HostAction::Forward {
            socket,
            worker_pid,
            operation,
            payload,
            remove_session,
        } => {
            let result = call_socket(&socket, &operation, payload);
            if let Some(session_id) = remove_session {
                host.lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sessions
                    .remove(&session_id);
                unsafe {
                    libc::kill(worker_pid as libc::pid_t, libc::SIGTERM);
                }
                let _ = fs::remove_file(&socket);
            }
            result
        }
    };
    let response = ResponseEnvelope {
        version: PROTOCOL_VERSION,
        request_id: request.request_id,
        result,
    };
    write_frame(&mut stream, &response)?;
    Ok(())
}

fn run_host(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let socket = args.socket.ok_or("missing --socket")?;
    let runtime_dir = args
        .runtime_dir
        .unwrap_or_else(|| PathBuf::from(format!("{socket}.runtime")));
    if UnixStream::connect(&socket).is_ok() {
        return Err(format!("qwe-host is already running at {socket}").into());
    }
    let _ = fs::remove_file(&socket);
    let session_result = unsafe { libc::setsid() };
    if session_result < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
        return Err(std::io::Error::last_os_error().into());
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let host = Arc::new(Mutex::new(Host::recover(runtime_dir)?));
    for stream in listener.incoming() {
        let host = Arc::clone(&host);
        thread::spawn(move || {
            if let Err(failure) = serve_host(stream?, &host) {
                eprintln!("host protocol error: {failure}");
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.worker_socket.is_some() {
        run_worker(args)
    } else {
        run_host(args)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_registration_fences_old_epoch() {
        let runtime_dir = std::env::temp_dir().join(format!(
            "qwe-host-test-{}-{}",
            std::process::id(),
            request_id("epoch")
        ));
        fs::create_dir_all(&runtime_dir).unwrap();
        let mut host = Host {
            sessions: HashMap::new(),
            epoch_file: runtime_dir.join("epoch"),
            runtime_dir: runtime_dir.clone(),
            next_epoch: 0,
            active_epoch: 0,
        };
        let first = match host.dispatch(&RequestEnvelope {
            version: PROTOCOL_VERSION,
            request_id: "one".into(),
            idempotency_key: None,
            host_epoch: None,
            operation: "HOST_REGISTER".into(),
            payload: json!({}),
        }) {
            HostAction::Done(Ok(value)) => value["epoch"].as_u64().unwrap(),
            _ => panic!("unexpected registration response"),
        };
        let second = match host.dispatch(&RequestEnvelope {
            version: PROTOCOL_VERSION,
            request_id: "two".into(),
            idempotency_key: None,
            host_epoch: None,
            operation: "HOST_REGISTER".into(),
            payload: json!({}),
        }) {
            HostAction::Done(Ok(value)) => value["epoch"].as_u64().unwrap(),
            _ => panic!("unexpected registration response"),
        };
        assert!(second > first);
        assert!(matches!(
            host.dispatch(&RequestEnvelope {
                version: PROTOCOL_VERSION,
                request_id: "stale".into(),
                idempotency_key: None,
                host_epoch: Some(first),
                operation: "HOST_LIST".into(),
                payload: json!({}),
            }),
            HostAction::Done(Err(error)) if error.code == "stale_epoch"
        ));
        let _ = fs::remove_dir_all(runtime_dir);
    }
}
