mod resources;

use clap::Parser;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use qwe_core::{ProcessIdentity, ResourcePolicy};
use qwe_protocol::{
    ATTACH_OUTPUT_QUEUE_BYTES, ATTACH_PROTOCOL_VERSION, PROTOCOL_VERSION, ProtocolErrorBody,
    RequestEnvelope, ResponseEnvelope, attach, read_frame, validate_version, verify_same_uid,
    write_frame,
};
use qwe_runtime::{
    BoundedJournal, CursorStyle as RuntimeCursorStyle,
    MouseProtocolEncoding as RuntimeMouseProtocolEncoding,
    MouseProtocolMode as RuntimeMouseProtocolMode, TerminalScreen, TerminalSnapshot,
    process_identity,
};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const JOURNAL_LIMIT: usize = 16 * 1024 * 1024;
const ATTACH_TAIL_BYTES: usize = 1024 * 1024;
const TERMINAL_SCROLLBACK_LINES: usize = 2000;
const TASK_STOP_GRACE: Duration = Duration::from_secs(1);
const TASK_STOP_WAIT: Duration = Duration::from_secs(1);
const ATTACH_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const ATTACH_WRITE_STALL: Duration = Duration::from_secs(5);

#[derive(Parser)]
struct Args {
    #[arg(long, hide = true)]
    worker_socket: PathBuf,
    #[arg(long, hide = true)]
    worker_session_id: String,
    #[arg(long, hide = true, default_value = "sh")]
    worker_program: String,
    #[arg(long = "worker-arg", hide = true, allow_hyphen_values = true)]
    worker_args: Vec<String>,
    #[arg(long, hide = true)]
    worker_cpu_seconds: Option<u64>,
    #[arg(long, hide = true)]
    worker_cpu_quota_percent: Option<u32>,
    #[arg(long, hide = true)]
    worker_memory_high_bytes: Option<u64>,
    #[arg(long, hide = true)]
    worker_memory_bytes: Option<u64>,
    #[arg(long, hide = true)]
    worker_memory_swap_max_bytes: Option<u64>,
    #[arg(long, hide = true)]
    worker_tasks_max: Option<u64>,
    #[arg(long, hide = true)]
    worker_credential_fd: Option<i32>,
    #[arg(long, hide = true)]
    worker_systemd_scope: bool,
}

struct WorkerSession {
    pid: u32,
    identity: ProcessIdentity,
    worker_identity: ProcessIdentity,
    resources: resources::ResourceController,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    stream: Mutex<TerminalStream>,
    active_attachment: Mutex<Option<ActiveAttachment>>,
    next_attachment_id: std::sync::atomic::AtomicU64,
    running: AtomicBool,
    exit_code: Mutex<Option<i32>>,
    exit_signal: Mutex<Option<String>>,
}

struct TerminalStream {
    journal: BoundedJournal,
    terminal: TerminalScreen,
    attachments: HashMap<u64, Arc<AttachmentQueue>>,
}

struct ActiveAttachment {
    id: u64,
}

#[derive(Debug)]
enum AttachmentEvent {
    Output { stream_offset: u64, data: Vec<u8> },
    Resized(attach::Resized),
    Exited(attach::ProcessExited),
    Error(attach::AttachError),
}

struct AttachmentQueueState {
    events: VecDeque<AttachmentEvent>,
    accounted_output_bytes: usize,
    closed: bool,
    overflowed: bool,
}

struct AttachmentQueue {
    state: Mutex<AttachmentQueueState>,
    changed: Condvar,
    limit: usize,
}

struct AttachmentRegistration {
    id: u64,
    queue: Arc<AttachmentQueue>,
    accepted: attach::AttachAccepted,
    snapshot: attach::TerminalSnapshot,
}

type TopologyInfo = (u32, PathBuf);
type TopologyReceiver = mpsc::Receiver<Result<TopologyInfo, String>>;

fn error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
    }
}

fn reclaim_token() -> Result<String, std::io::Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|failure| std::io::Error::other(format!("random source failed: {failure}")))?;
    Ok(hex_encode(&bytes))
}

fn worker_attach_socket(runtime_dir: &Path, session_id: &str) -> PathBuf {
    runtime_dir.join(format!("{}.a", hex_encode(session_id.as_bytes())))
}

impl AttachmentQueue {
    fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(AttachmentQueueState {
                events: VecDeque::new(),
                accounted_output_bytes: 0,
                closed: false,
                overflowed: false,
            }),
            changed: Condvar::new(),
            limit,
        }
    }

    fn push_output(&self, stream_offset: u64, data: Vec<u8>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed || state.overflowed {
            return;
        }
        if state.accounted_output_bytes.saturating_add(data.len()) > self.limit {
            state.events.clear();
            state.accounted_output_bytes = data.len();
            state.overflowed = true;
            state.closed = true;
            self.changed.notify_all();
            return;
        }
        state.accounted_output_bytes += data.len();
        state.events.push_back(AttachmentEvent::Output {
            stream_offset,
            data,
        });
        self.changed.notify_one();
    }

    fn push_control(&self, event: AttachmentEvent) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed || state.overflowed {
            return;
        }
        state.events.push_back(event);
        self.changed.notify_one();
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        self.changed.notify_all();
    }

    fn next_event(&self) -> Result<Option<AttachmentEvent>, ()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if state.overflowed {
                return Err(());
            }
            if let Some(event) = state.events.pop_front() {
                return Ok(Some(event));
            }
            if state.closed {
                return Ok(None);
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn complete_output(&self, bytes: usize) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.accounted_output_bytes = state.accounted_output_bytes.saturating_sub(bytes);
    }
}

impl TerminalStream {
    fn new(rows: u16, columns: u16) -> Result<Self, std::io::Error> {
        Ok(Self {
            journal: BoundedJournal::new(JOURNAL_LIMIT).map_err(|failure| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, failure.to_string())
            })?,
            terminal: TerminalScreen::new(rows, columns, TERMINAL_SCROLLBACK_LINES),
            attachments: HashMap::new(),
        })
    }

    fn append_output(&mut self, data: &[u8]) {
        let stream_offset = self.journal.total_written();
        self.journal.append(data);
        self.terminal.process(data);
        for queue in self.attachments.values() {
            queue.push_output(stream_offset, data.to_vec());
        }
    }

    fn stream_offset(&self) -> u64 {
        self.journal.total_written()
    }
}

fn protocol_snapshot(
    snapshot: TerminalSnapshot,
    next_stream_offset: u64,
) -> attach::TerminalSnapshot {
    let cursor_style = match snapshot.cursor_style {
        RuntimeCursorStyle::Default => attach::CursorStyle::Default,
        RuntimeCursorStyle::BlinkBlock => attach::CursorStyle::BlinkBlock,
        RuntimeCursorStyle::SteadyBlock => attach::CursorStyle::SteadyBlock,
        RuntimeCursorStyle::BlinkUnderline => attach::CursorStyle::BlinkUnderline,
        RuntimeCursorStyle::SteadyUnderline => attach::CursorStyle::SteadyUnderline,
        RuntimeCursorStyle::BlinkVerticalBar => attach::CursorStyle::BlinkVerticalBar,
        RuntimeCursorStyle::SteadyVerticalBar => attach::CursorStyle::SteadyVerticalBar,
    };
    let mouse_protocol_mode = match snapshot.mouse_protocol_mode {
        RuntimeMouseProtocolMode::None => attach::MouseProtocolMode::None,
        RuntimeMouseProtocolMode::Press => attach::MouseProtocolMode::Press,
        RuntimeMouseProtocolMode::PressRelease => attach::MouseProtocolMode::PressRelease,
        RuntimeMouseProtocolMode::ButtonMotion => attach::MouseProtocolMode::ButtonMotion,
        RuntimeMouseProtocolMode::AnyMotion => attach::MouseProtocolMode::AnyMotion,
    };
    let mouse_protocol_encoding = match snapshot.mouse_protocol_encoding {
        RuntimeMouseProtocolEncoding::Default => attach::MouseProtocolEncoding::Default,
        RuntimeMouseProtocolEncoding::Utf8 => attach::MouseProtocolEncoding::Utf8,
        RuntimeMouseProtocolEncoding::Sgr => attach::MouseProtocolEncoding::Sgr,
    };
    attach::TerminalSnapshot {
        rows: snapshot.rows,
        columns: snapshot.columns,
        cursor_row: snapshot.cursor_row,
        cursor_column: snapshot.cursor_column,
        cursor_visible: snapshot.cursor_visible,
        cursor_style,
        alternate_screen: snapshot.alternate_screen,
        application_cursor: snapshot.application_cursor,
        application_keypad: snapshot.application_keypad,
        bracketed_paste: snapshot.bracketed_paste,
        mouse_protocol_mode,
        mouse_protocol_encoding,
        screen_text: snapshot.screen_text,
        next_stream_offset,
    }
}

fn read_credential(fd: i32) -> std::io::Result<Zeroizing<Vec<u8>>> {
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut secret = Zeroizing::new(Vec::new());
    file.take(16 * 1024 + 1).read_to_end(&mut secret)?;
    if secret.len() > 16 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "credential exceeds the 16 KiB limit",
        ));
    }
    Ok(secret)
}

fn start_credential_broker(
    runtime_dir: &Path,
    secret: Zeroizing<Vec<u8>>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random).map_err(|failure| failure.to_string())?;
    let path = runtime_dir.join(format!(
        "cred-{}-{}.sock",
        std::process::id(),
        hex_encode(&random)
    ));
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let broker_path = path.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.write_all(&secret);
                    let _ = stream.flush();
                    break;
                }
                Err(failure) if failure.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        let _ = fs::remove_file(&broker_path);
    });
    Ok(path)
}

fn start_topology_listener(
    runtime_dir: &Path,
) -> Result<(PathBuf, TopologyReceiver), Box<dyn std::error::Error>> {
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random).map_err(|failure| failure.to_string())?;
    let path = runtime_dir.join(format!(
        "topology-{}-{}.sock",
        std::process::id(),
        hex_encode(&random)
    ));
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let (sender, receiver) = mpsc::channel();
    let cleanup_path = path.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let result = loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    break (|| {
                        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                        let mut payload = String::new();
                        stream.read_to_string(&mut payload)?;
                        let value: Value = serde_json::from_str(&payload)?;
                        let pid = value
                            .get("pid")
                            .and_then(Value::as_u64)
                            .and_then(|pid| u32::try_from(pid).ok())
                            .ok_or_else(|| "topology response has no PID".to_string())?;
                        let cgroup = value
                            .get("cgroup")
                            .and_then(Value::as_str)
                            .filter(|value| !value.is_empty())
                            .ok_or_else(|| "topology response has no cgroup".to_string())?;
                        Ok((pid, PathBuf::from(cgroup)))
                    })()
                    .map_err(|failure: Box<dyn std::error::Error>| failure.to_string());
                }
                Err(failure) if failure.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        break Err("topology handshake timed out".to_string());
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(failure) => break Err(failure.to_string()),
            }
        };
        let _ = sender.send(result);
        let _ = fs::remove_file(cleanup_path);
    });
    Ok((path, receiver))
}

impl WorkerSession {
    fn create(
        program: &str,
        arguments: &[String],
        resource_policy: ResourcePolicy,
        credential_fd: Option<i32>,
        runtime_dir: &Path,
        use_systemd_scope: bool,
    ) -> Result<Arc<Self>, Box<dyn std::error::Error>> {
        let pair = native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let credential = credential_fd.map(read_credential).transpose()?;
        let credential_socket = credential
            .map(|secret| start_credential_broker(runtime_dir, secret))
            .transpose()?;
        let topology = if use_systemd_scope {
            Some(start_topology_listener(runtime_dir)?)
        } else {
            None
        };
        let executable = std::env::current_exe()?.with_file_name("qwe-session-exec");
        let mut command = if let Some((topology_socket, _)) = &topology {
            let mut random = [0_u8; 8];
            getrandom::fill(&mut random).map_err(|failure| failure.to_string())?;
            let unit = format!("qwe-session-{}-{}", std::process::id(), hex_encode(&random));
            let mut command = CommandBuilder::new("/usr/bin/systemd-run");
            command.args(["--user", "--scope", "--quiet", "--collect", "--unit", &unit]);
            command.args([
                "--property=MemoryAccounting=yes",
                "--property=TasksAccounting=yes",
                "--property=CPUAccounting=yes",
            ]);
            if let Some(bytes) = resource_policy.memory_high_bytes {
                command.arg(format!("--property=MemoryHigh={bytes}"));
            }
            if let Some(bytes) = resource_policy.memory_max_bytes {
                command.arg(format!("--property=MemoryMax={bytes}"));
            }
            if let Some(bytes) = resource_policy.memory_swap_max_bytes {
                command.arg(format!("--property=MemorySwapMax={bytes}"));
            } else if resource_policy.memory_max_bytes.is_some()
                || resource_policy.memory_high_bytes.is_some()
            {
                command.arg("--property=MemorySwapMax=0");
            }
            if let Some(tasks) = resource_policy.tasks_max {
                command.arg(format!("--property=TasksMax={tasks}"));
            }
            if let Some(percent) = resource_policy.cpu_quota_percent {
                command.arg(format!("--property=CPUQuota={percent}%"));
            }
            command.arg("--");
            command.arg(executable);
            command.arg("--topology-socket");
            command.arg(topology_socket);
            if let Some(socket) = &credential_socket {
                command.arg("--credential-socket");
                command.arg(socket);
            }
            command.arg(program);
            command.args(arguments.iter());
            command
        } else if credential_socket.is_some() {
            let mut command = CommandBuilder::new(executable);
            if let Some(socket) = &credential_socket {
                command.arg("--credential-socket");
                command.arg(socket);
            }
            command.arg(program);
            command.args(arguments.iter());
            command
        } else {
            let mut command = CommandBuilder::new(program);
            command.args(arguments.iter());
            command
        };
        command.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(command);
        let mut child = child?;
        let (pid, cgroup_dir) = if let Some((_, receiver)) = topology {
            match receiver.recv_timeout(Duration::from_secs(3)) {
                Ok(Ok((pid, cgroup))) => (pid, Some(cgroup)),
                Ok(Err(failure)) => {
                    let _ = child.kill();
                    return Err(format!("Session topology handshake failed: {failure}").into());
                }
                Err(failure) => {
                    let _ = child.kill();
                    return Err(format!("Session topology handshake timed out: {failure}").into());
                }
            }
        } else {
            (
                child
                    .process_id()
                    .ok_or("session process has no PID".to_string())?,
                None,
            )
        };
        let identity = process_identity(pid)?;
        let pgid = identity.pgid;
        drop(pair.slave);
        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        let master = pair.master;
        let initial_policy = resource_policy.clone();
        let resources = resources::ResourceController::new(resource_policy, cgroup_dir);
        resources.update(initial_policy)?;
        let session = Arc::new(Self {
            pid,
            identity,
            worker_identity: process_identity(std::process::id())?,
            resources,
            writer: Mutex::new(writer),
            master: Mutex::new(master),
            stream: Mutex::new(TerminalStream::new(24, 80)?),
            active_attachment: Mutex::new(None),
            next_attachment_id: std::sync::atomic::AtomicU64::new(1),
            running: AtomicBool::new(true),
            exit_code: Mutex::new(None),
            exit_signal: Mutex::new(None),
        });
        session.resources.start_monitor(pid, pgid);
        let reader_session = Arc::clone(&session);
        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                reader_session
                    .stream
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .append_output(&buffer[..count]);
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
            reader_session.resources.capture_final_sample();
            reader_session.resources.cleanup_cgroup();
            reader_session.running.store(false, Ordering::SeqCst);
            let exit_code = *reader_session
                .exit_code
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let exit_signal = reader_session
                .exit_signal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let stream = reader_session
                .stream
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let event = attach::ProcessExited {
                protocol_version: ATTACH_PROTOCOL_VERSION,
                exit_code: exit_code.and_then(|value| u8::try_from(value).ok()),
                exit_signal,
                stream_offset: stream.stream_offset(),
            };
            for queue in stream.attachments.values() {
                queue.push_control(AttachmentEvent::Exited(event.clone()));
                queue.close();
            }
        });
        Ok(session)
    }

    fn status(&self, session_id: &str) -> Value {
        let stream = self
            .stream
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
            "worker_identity": self.worker_identity,
            "pid": self.pid,
            "pgid": self.identity.pgid,
            "start_token": self.identity.start_token,
            "process_identity": self.identity,
            "running": self.running.load(Ordering::SeqCst),
            "state": if self.running.load(Ordering::SeqCst) { "running" } else { "exited" },
            "exit_code": exit_code,
            "exit_signal": exit_signal,
            "resource_policy": self.resources.policy_json(),
            "resource_metrics": self.resources.metrics_json(),
            "truncated_from": stream.journal.truncated_from(),
            "total_written": stream.journal.total_written()
        })
    }

    fn ping(&self, session_id: &str) -> Value {
        let running = self.running.load(Ordering::SeqCst);
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
            "worker_identity": self.worker_identity,
            "pid": self.pid,
            "pgid": self.identity.pgid,
            "start_token": self.identity.start_token,
            "process_identity": self.identity,
            "running": running,
            "state": if running { "running" } else { "exited" },
            "exit_code": exit_code,
            "exit_signal": exit_signal,
            "resource_policy": self.resources.policy_json(),
            "resource_metrics": self.resources.metrics_json()
        })
    }

    fn snapshot(&self, session_id: &str) -> Value {
        let running = self.running.load(Ordering::SeqCst);
        let stream = self
            .stream
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
        let tail = stream.journal.tail(ATTACH_TAIL_BYTES);
        let terminal = stream.terminal.snapshot();
        json!({
            "session_id": session_id,
            "worker_pid": std::process::id(),
            "worker_identity": self.worker_identity,
            "pid": self.pid,
            "pgid": self.identity.pgid,
            "start_token": self.identity.start_token,
            "process_identity": self.identity,
            "running": running,
            "state": if running { "running" } else { "exited" },
            "exit_code": exit_code,
            "exit_signal": exit_signal,
            "resource_policy": self.resources.policy_json(),
            "resource_metrics": self.resources.metrics_json(),
            "screen": terminal.screen_text,
            "terminal": protocol_snapshot(terminal, stream.stream_offset()),
            "journal_hex": hex_encode(&tail),
            "snapshot_bytes": tail.len(),
            "truncated_from": stream.journal.truncated_from(),
            "total_written": stream.journal.total_written()
        })
    }

    fn input(&self, session_id: &str, payload: &Value) -> Result<Value, ProtocolErrorBody> {
        let data = payload
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| error("invalid_request", "missing input data"))?;
        if data.len() > 1024 * 1024 {
            return Err(error(
                "input_too_large",
                "worker input exceeds the 1 MiB control limit",
            ));
        }
        self.writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write_all(data.as_bytes())
            .map_err(|failure| error("internal", failure.to_string()))?;
        Ok(self.status(session_id))
    }

    fn update_resources(
        &self,
        session_id: &str,
        payload: &Value,
    ) -> Result<Value, ProtocolErrorBody> {
        let policy = payload
            .get("resource_policy")
            .cloned()
            .ok_or_else(|| error("invalid_request", "missing resource_policy"))
            .and_then(|value| {
                serde_json::from_value::<ResourcePolicy>(value)
                    .map_err(|failure| error("invalid_request", failure.to_string()))
            })?;
        self.resources
            .update(policy)
            .map_err(|failure| error("resource_apply_failed", failure.to_string()))?;
        Ok(self.status(session_id))
    }

    fn resize(&self, rows: u16, columns: u16) -> Result<attach::Resized, ProtocolErrorBody> {
        if rows == 0 || columns == 0 {
            return Err(error(
                "invalid_request",
                "terminal rows and columns must be positive",
            ));
        }
        let mut stream = self
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.master
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resize(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|failure| error("resize_failed", failure.to_string()))?;
        stream.terminal.resize(rows, columns);
        let resized = attach::Resized {
            protocol_version: ATTACH_PROTOCOL_VERSION,
            rows,
            columns,
            stream_offset: stream.stream_offset(),
        };
        for queue in stream.attachments.values() {
            queue.push_control(AttachmentEvent::Resized(resized.clone()));
        }
        Ok(resized)
    }

    fn register_attachment(
        &self,
        open: &attach::AttachOpen,
    ) -> Result<AttachmentRegistration, ProtocolErrorBody> {
        if open.mode == attach::AttachMode::ReadOnly {
            return Err(error(
                "unsupported_mode",
                "read_only attach mode is reserved and is not supported in V0",
            ));
        }
        if open.rows == 0 || open.columns == 0 {
            return Err(error(
                "invalid_request",
                "terminal rows and columns must be positive",
            ));
        }
        let id = self.next_attachment_id.fetch_add(1, Ordering::SeqCst);
        let mut active = self
            .active_attachment
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active.is_some() {
            return Err(error(
                "already_attached",
                "the session already has an exclusive attachment",
            ));
        }
        *active = Some(ActiveAttachment { id });
        let token = match reclaim_token() {
            Ok(token) => token,
            Err(failure) => {
                *active = None;
                return Err(error("internal", failure.to_string()));
            }
        };
        drop(active);

        let registration = (|| {
            let mut stream = self
                .stream
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let current = stream.terminal.snapshot();
            if (current.rows, current.columns) != (open.rows, open.columns) {
                self.master
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .resize(PtySize {
                        rows: open.rows,
                        cols: open.columns,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .map_err(|failure| error("resize_failed", failure.to_string()))?;
                stream.terminal.resize(open.rows, open.columns);
            }
            let queue = Arc::new(AttachmentQueue::new(ATTACH_OUTPUT_QUEUE_BYTES));
            stream.attachments.insert(id, Arc::clone(&queue));
            let snapshot = stream.terminal.snapshot();
            let stream_offset = stream.stream_offset();
            let accepted = attach::AttachAccepted {
                protocol_version: ATTACH_PROTOCOL_VERSION,
                session_id: open.session_id.clone(),
                owner: open.owner.clone(),
                generation: 0,
                reclaim_token: token,
                rows: snapshot.rows,
                columns: snapshot.columns,
                stream_offset,
            };
            Ok(AttachmentRegistration {
                id,
                queue,
                accepted,
                snapshot: protocol_snapshot(snapshot, stream_offset),
            })
        })();
        if registration.is_err() {
            let mut active = self
                .active_attachment
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if active
                .as_ref()
                .is_some_and(|attachment| attachment.id == id)
            {
                *active = None;
            }
        }
        registration
    }

    fn unregister_attachment(&self, id: u64) {
        let mut active = self
            .active_attachment
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .as_ref()
            .is_some_and(|attachment| attachment.id == id)
        {
            *active = None;
        }
        drop(active);
        self.stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .attachments
            .remove(&id);
    }

    fn signal_process_group(&self, signal: libc::c_int) -> Result<(), ProtocolErrorBody> {
        let result = unsafe { libc::kill(-(self.pid as libc::pid_t), signal) };
        if result == 0 {
            return Ok(());
        }
        let failure = std::io::Error::last_os_error();
        if failure.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(error(
            "signal_failed",
            format!(
                "failed to signal process group {}: {failure}",
                self.identity.pgid
            ),
        ))
    }

    fn wait_for_exit(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.running.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        !self.running.load(Ordering::SeqCst)
    }

    fn process_group_exists(&self) -> bool {
        let result = unsafe { libc::kill(-(self.identity.pgid as libc::pid_t), 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    fn wait_for_process_group_exit(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.process_group_exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        !self.process_group_exists()
    }

    fn stop_task(&self, session_id: &str) -> Result<Value, ProtocolErrorBody> {
        if self.process_group_exists() {
            self.signal_process_group(libc::SIGTERM)?;
            if !self.wait_for_process_group_exit(TASK_STOP_GRACE) {
                self.signal_process_group(libc::SIGKILL)?;
                if !self.wait_for_process_group_exit(TASK_STOP_WAIT) {
                    return Err(error(
                        "task_stop_timeout",
                        format!(
                            "task process group {} did not exit after SIGKILL",
                            self.identity.pgid
                        ),
                    ));
                }
            }
        }
        if !self.wait_for_exit(TASK_STOP_WAIT) {
            return Err(error(
                "task_status_timeout",
                "task process group exited but terminal status did not settle",
            ));
        }
        Ok(self.status(session_id))
    }

    fn close(&self, session_id: &str) -> Result<Value, ProtocolErrorBody> {
        let status = self.stop_task(session_id)?;
        Ok(json!({
            "session_id": session_id,
            "state": "closed",
            "exit_code": status.get("exit_code"),
            "exit_signal": status.get("exit_signal")
        }))
    }

    fn kill(&self, session_id: &str) -> Result<Value, ProtocolErrorBody> {
        if self.process_group_exists() {
            self.signal_process_group(libc::SIGKILL)?;
            if !self.wait_for_process_group_exit(TASK_STOP_WAIT) {
                return Err(error(
                    "task_kill_timeout",
                    format!(
                        "task process group {} did not exit after SIGKILL",
                        self.identity.pgid
                    ),
                ));
            }
        }
        if !self.wait_for_exit(TASK_STOP_WAIT) {
            return Err(error(
                "task_status_timeout",
                "task process group exited but terminal status did not settle",
            ));
        }
        Ok(self.status(session_id))
    }
}

fn attach_error(code: &str, message: impl Into<String>) -> attach::AttachError {
    attach::AttachError {
        protocol_version: ATTACH_PROTOCOL_VERSION,
        code: code.into(),
        message: message.into(),
    }
}

fn write_attach_frame(
    stream: &mut UnixStream,
    frame_type: attach::FrameType,
    payload: &[u8],
) -> std::io::Result<()> {
    let header = attach::FrameHeader {
        frame_type,
        payload_length: payload.len(),
    }
    .encode()
    .map_err(|failure| std::io::Error::other(failure.to_string()))?;
    let mut frame = Vec::with_capacity(header.len() + payload.len());
    frame.extend_from_slice(&header);
    frame.extend_from_slice(payload);

    let mut written = 0;
    let mut last_progress = Instant::now();
    while written < frame.len() {
        match stream.write(&frame[written..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "attach socket accepted zero bytes",
                ));
            }
            Ok(count) => {
                written += count;
                last_progress = Instant::now();
            }
            Err(failure)
                if matches!(
                    failure.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if last_progress.elapsed() >= ATTACH_WRITE_STALL {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "attach client made no output progress for five seconds",
                    ));
                }
            }
            Err(failure) => return Err(failure),
        }
    }
    Ok(())
}

fn write_attach_json(
    stream: &mut UnixStream,
    frame_type: attach::FrameType,
    value: &impl serde::Serialize,
) -> std::io::Result<()> {
    let payload = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    write_attach_frame(stream, frame_type, &payload)
}

fn write_attach_event(
    stream: &mut UnixStream,
    queue: &AttachmentQueue,
    event: AttachmentEvent,
) -> std::io::Result<bool> {
    match event {
        AttachmentEvent::Output {
            stream_offset,
            data,
        } => {
            let mut payload = Vec::with_capacity(size_of::<u64>() + data.len());
            payload.extend_from_slice(&stream_offset.to_be_bytes());
            payload.extend_from_slice(&data);
            write_attach_frame(stream, attach::FrameType::Output, &payload)?;
            queue.complete_output(data.len());
            Ok(true)
        }
        AttachmentEvent::Resized(resized) => {
            write_attach_json(stream, attach::FrameType::Resized, &resized)?;
            Ok(true)
        }
        AttachmentEvent::Exited(exited) => {
            write_attach_json(stream, attach::FrameType::ProcessExited, &exited)?;
            Ok(false)
        }
        AttachmentEvent::Error(failure) => {
            write_attach_json(stream, attach::FrameType::Error, &failure)?;
            Ok(false)
        }
    }
}

fn serve_attachment_input(
    mut stream: UnixStream,
    session: Arc<WorkerSession>,
    queue: Arc<AttachmentQueue>,
) {
    loop {
        let frame = match attach::read_frame(&mut stream) {
            Ok(frame) => frame,
            Err(attach::AttachIoError::Codec(failure)) => {
                queue.push_control(AttachmentEvent::Error(attach_error(
                    "unknown_frame_type",
                    failure.to_string(),
                )));
                queue.close();
                return;
            }
            Err(_) => {
                queue.close();
                return;
            }
        };
        let result =
            (|| match frame.frame_type {
                attach::FrameType::Input => session
                    .writer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .write_all(&frame.payload)
                    .map_err(|failure| attach_error("worker_protocol_error", failure.to_string())),
                attach::FrameType::Resize => {
                    let resize = attach::decode_json::<attach::Resize>(&frame.payload).map_err(
                        |failure| attach_error("worker_protocol_error", failure.to_string()),
                    )?;
                    if resize.protocol_version != ATTACH_PROTOCOL_VERSION {
                        return Err(attach_error(
                            "incompatible_version",
                            format!(
                                "attach protocol {} is incompatible with {}",
                                resize.protocol_version, ATTACH_PROTOCOL_VERSION
                            ),
                        ));
                    }
                    session
                        .resize(resize.rows, resize.columns)
                        .map(|_| ())
                        .map_err(|failure| attach_error(&failure.code, failure.message))
                }
                attach::FrameType::Ack => attach::decode_json::<attach::Ack>(&frame.payload)
                    .map(|_| ())
                    .map_err(|failure| attach_error("worker_protocol_error", failure.to_string())),
                attach::FrameType::Detach => {
                    let detach = attach::decode_json::<attach::Detach>(&frame.payload).map_err(
                        |failure| attach_error("worker_protocol_error", failure.to_string()),
                    )?;
                    if detach.hold_ttl_ms > 0 {
                        return Err(attach_error(
                            "unsupported_mode",
                            "detached holds are scheduled for Stage 3",
                        ));
                    }
                    Ok(())
                }
                _ => Err(attach_error(
                    "unknown_frame_type",
                    format!(
                        "{} is not a valid client-to-server frame",
                        frame.frame_type.name()
                    ),
                )),
            })();
        match result {
            Ok(()) => {
                if frame.frame_type == attach::FrameType::Detach {
                    queue.close();
                    return;
                }
            }
            Err(failure) => {
                queue.push_control(AttachmentEvent::Error(failure));
                queue.close();
                return;
            }
        }
    }
}

fn serve_worker_attachment(
    mut stream: UnixStream,
    session: Arc<WorkerSession>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if verify_same_uid(&stream).is_err() {
        return Ok(());
    }
    let frame = match attach::read_frame(&mut stream) {
        Ok(frame) => frame,
        Err(failure) => {
            let _ = attach::write_json_frame(
                &mut stream,
                attach::FrameType::Error,
                &attach_error("unknown_frame_type", failure.to_string()),
            );
            return Ok(());
        }
    };
    if frame.frame_type != attach::FrameType::AttachOpen {
        let _ = attach::write_json_frame(
            &mut stream,
            attach::FrameType::Error,
            &attach_error(
                "unknown_frame_type",
                "the first attach frame must be ATTACH_OPEN",
            ),
        );
        return Ok(());
    }
    let open = match attach::decode_json::<attach::AttachOpen>(&frame.payload) {
        Ok(open) => open,
        Err(failure) => {
            let _ = attach::write_json_frame(
                &mut stream,
                attach::FrameType::Error,
                &attach_error("worker_protocol_error", failure.to_string()),
            );
            return Ok(());
        }
    };
    if open.protocol_version != ATTACH_PROTOCOL_VERSION {
        let _ = attach::write_json_frame(
            &mut stream,
            attach::FrameType::Error,
            &attach_error(
                "incompatible_version",
                format!(
                    "attach protocol {} is incompatible with {}",
                    open.protocol_version, ATTACH_PROTOCOL_VERSION
                ),
            ),
        );
        return Ok(());
    }
    let registration = match session.register_attachment(&open) {
        Ok(registration) => registration,
        Err(failure) => {
            let _ = attach::write_json_frame(
                &mut stream,
                attach::FrameType::Error,
                &attach_error(&failure.code, failure.message),
            );
            return Ok(());
        }
    };
    stream.set_write_timeout(Some(ATTACH_WRITE_TIMEOUT))?;
    if let Err(failure) = attach::write_json_frame(
        &mut stream,
        attach::FrameType::AttachAccepted,
        &registration.accepted,
    )
    .and_then(|()| {
        attach::write_json_frame(
            &mut stream,
            attach::FrameType::Snapshot,
            &registration.snapshot,
        )
    }) {
        session.unregister_attachment(registration.id);
        return Err(failure.into());
    }

    let input_stream = stream.try_clone()?;
    let input_session = Arc::clone(&session);
    let input_queue = Arc::clone(&registration.queue);
    thread::spawn(move || {
        serve_attachment_input(input_stream, input_session, input_queue);
    });

    loop {
        let event = match registration.queue.next_event() {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(()) => {
                let _ = write_attach_json(
                    &mut stream,
                    attach::FrameType::Error,
                    &attach_error(
                        "output_backpressure",
                        "attachment output queue exceeded 1 MiB",
                    ),
                );
                break;
            }
        };
        match write_attach_event(&mut stream, &registration.queue, event) {
            Ok(true) => {}
            Ok(false) => break,
            Err(failure) => {
                eprintln!("attach transport error: {failure}");
                break;
            }
        }
    }

    registration.queue.close();
    session.unregister_attachment(registration.id);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    Ok(())
}

fn worker_handle(
    session: &Arc<WorkerSession>,
    operation: &str,
    session_id: &str,
    payload: &Value,
) -> Result<Value, ProtocolErrorBody> {
    match operation {
        "PING" => Ok(session.ping(session_id)),
        "WORKER_HELLO" | "SESSION_STATUS" => Ok(session.status(session_id)),
        "SESSION_ATTACH" => Ok(session.snapshot(session_id)),
        "SESSION_INPUT" => session.input(session_id, payload),
        "SESSION_RESOURCE_UPDATE" => session.update_resources(session_id, payload),
        "TASK_STOP" => session.stop_task(session_id),
        "SESSION_CLOSE" => session.close(session_id),
        "TASK_KILL" | "SESSION_KILL" => session.kill(session_id),
        _ => Err(error("unsupported_operation", operation)),
    }
}

fn serve_worker(stream: UnixStream, session: &Arc<WorkerSession>, session_id: &str) {
    if verify_same_uid(&stream).is_err() {
        return;
    }
    let mut reader = match stream.try_clone() {
        Ok(reader) => reader,
        Err(_) => return,
    };
    let writer = Arc::new(Mutex::new(stream));
    loop {
        let request: RequestEnvelope = match read_frame(&mut reader) {
            Ok(request) => request,
            Err(qwe_protocol::IoFrameError::Io(failure))
                if matches!(
                    failure.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::BrokenPipe
                ) =>
            {
                return;
            }
            Err(failure) => {
                eprintln!("worker control protocol error: {failure}");
                return;
            }
        };
        if let Err(failure) = validate_version(request.version) {
            eprintln!("worker control protocol error: {failure}");
            return;
        }
        let session = Arc::clone(session);
        let session_id = session_id.to_string();
        let writer = Arc::clone(&writer);
        thread::spawn(move || {
            let response = ResponseEnvelope {
                version: PROTOCOL_VERSION,
                request_id: request.request_id,
                result: worker_handle(&session, &request.operation, &session_id, &request.payload),
            };
            if let Err(failure) = write_frame(
                &mut *writer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
                &response,
            ) {
                eprintln!("worker control response error: {failure}");
            }
        });
    }
}

fn run_worker(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let socket = args.worker_socket;
    let session_id = args.worker_session_id;
    let runtime_dir = socket
        .parent()
        .expect("worker socket always has a runtime parent")
        .to_path_buf();
    if UnixStream::connect(&socket).is_ok() {
        return Err(format!("session worker already running at {}", socket.display()).into());
    }
    let _ = fs::remove_file(&socket);
    let attach_path = worker_attach_socket(
        socket
            .parent()
            .expect("worker socket always has a runtime parent"),
        &session_id,
    );
    let _ = fs::remove_file(&attach_path);
    let session_result = unsafe { libc::setsid() };
    if session_result < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
        return Err(std::io::Error::last_os_error().into());
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let session = WorkerSession::create(
        &args.worker_program,
        &args.worker_args,
        ResourcePolicy {
            memory_high_bytes: args.worker_memory_high_bytes,
            memory_max_bytes: args.worker_memory_bytes,
            memory_swap_max_bytes: args.worker_memory_swap_max_bytes,
            tasks_max: args.worker_tasks_max,
            cpu_quota_percent: args.worker_cpu_quota_percent,
            cpu_seconds: args.worker_cpu_seconds,
        },
        args.worker_credential_fd,
        &runtime_dir,
        args.worker_systemd_scope,
    )?;
    let control_session = Arc::clone(&session);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let session = Arc::clone(&control_session);
            let session_id = session_id.clone();
            thread::spawn(move || serve_worker(stream.unwrap(), &session, &session_id));
        }
    });

    let attach_listener = UnixListener::bind(&attach_path)?;
    fs::set_permissions(&attach_path, fs::Permissions::from_mode(0o600))?;
    for stream in attach_listener.incoming() {
        let session = Arc::clone(&session);
        thread::spawn(move || {
            if let Err(failure) = serve_worker_attachment(stream?, session) {
                eprintln!("worker attach error: {failure}");
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_worker(Args::parse())
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
    fn attachment_queue_overflow_is_isolated_per_attachment() {
        let slow = AttachmentQueue::new(4);
        let fast = AttachmentQueue::new(4);
        slow.push_output(0, b"abcd".to_vec());
        slow.push_output(4, b"e".to_vec());
        assert!(slow.next_event().is_err());

        fast.push_output(0, b"ok".to_vec());
        let event = fast.next_event().unwrap().unwrap();
        assert!(matches!(
            event,
            AttachmentEvent::Output {
                stream_offset: 0,
                data
            } if data == b"ok"
        ));
    }
}
