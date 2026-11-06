use clap::{Args, Parser};
use qwe_core::DEFAULT_LEASE_TTL_MS;
use qwe_protocol::{
    ATTACH_PROTOCOL_VERSION, PROTOCOL_VERSION, ProtocolErrorBody, RequestEnvelope,
    ResponseEnvelope, attach, read_frame, verify_same_uid, write_frame,
};
use serde_json::{Value, json};
use std::io::{IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const TERMINAL_RESET_SEQUENCE: &[u8] = b"\x1b[0m\x1b[0 q\x1b[?25h\x1b[?1l\x1b>\x1b[?7h\
      \x1b[?2004l\x1b[?2026l\x1b[?1049l\
      \x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\
      \x1b[<1u\x1b[=0u\x1b[>4;0m";

#[derive(Debug, Parser)]
#[command(version, about = "Quick Window Environment CLI")]
struct Arguments {
    #[arg(long, global = true, default_value = "/tmp/qwe-client.sock")]
    socket: String,
    #[arg(long, global = true, default_value = "local")]
    owner: String,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Parser)]
enum Command {
    Status,
    Ping,
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Lease {
        #[command(subcommand)]
        command: LeaseCommand,
    },
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    KillSession {
        id: String,
        #[arg(long)]
        generation: Option<u64>,
    },
    KillServer {
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Debug, Parser)]
enum SessionCommand {
    Create {
        id: String,
        #[arg(default_value = "sh")]
        program: String,
        #[arg(long)]
        credential: Option<String>,
        #[command(flatten)]
        resources: ResourceArgs,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    Limits {
        id: String,
        #[arg(long)]
        generation: Option<u64>,
        #[arg(long)]
        clear: bool,
        #[command(flatten)]
        resources: ResourceArgs,
    },
    Status {
        id: String,
    },
    Attach {
        id: String,
    },
    Snapshot {
        id: String,
    },
    Input {
        id: String,
        data: String,
    },
    Detach {
        id: String,
    },
    Close {
        id: String,
        #[arg(long)]
        generation: Option<u64>,
    },
}

#[derive(Debug, Args)]
struct ResourceArgs {
    #[arg(long)]
    cpu_seconds: Option<u64>,
    #[arg(long)]
    cpu_quota_percent: Option<u32>,
    #[arg(long)]
    memory_high_bytes: Option<u64>,
    #[arg(long = "memory-max", visible_alias = "memory-bytes")]
    memory_max_bytes: Option<u64>,
    #[arg(long)]
    memory_swap_max_bytes: Option<u64>,
    #[arg(long = "tasks-max", visible_alias = "process-limit")]
    tasks_max: Option<u64>,
}

impl ResourceArgs {
    fn policy_json(&self, clear: bool) -> Option<Value> {
        let mut policy = serde_json::Map::new();
        if clear {
            for field in [
                "cpu_seconds",
                "cpu_quota_percent",
                "memory_high_bytes",
                "memory_max_bytes",
                "memory_swap_max_bytes",
                "tasks_max",
            ] {
                policy.insert(field.into(), Value::Null);
            }
        }
        if let Some(value) = self.cpu_seconds {
            policy.insert("cpu_seconds".into(), json!(value));
        }
        if let Some(value) = self.cpu_quota_percent {
            policy.insert("cpu_quota_percent".into(), json!(value));
        }
        if let Some(value) = self.memory_high_bytes {
            policy.insert("memory_high_bytes".into(), json!(value));
        }
        if let Some(value) = self.memory_max_bytes {
            policy.insert("memory_max_bytes".into(), json!(value));
        }
        if let Some(value) = self.memory_swap_max_bytes {
            policy.insert("memory_swap_max_bytes".into(), json!(value));
        }
        if let Some(value) = self.tasks_max {
            policy.insert("tasks_max".into(), json!(value));
        }
        (!policy.is_empty()).then_some(Value::Object(policy))
    }
}

#[derive(Debug, Parser)]
enum LeaseCommand {
    Claim {
        session_id: String,
        owner: String,
        #[arg(long, default_value_t = DEFAULT_LEASE_TTL_MS)]
        ttl_ms: i64,
    },
    Touch {
        session_id: String,
        owner: String,
        generation: u64,
        #[arg(long, default_value_t = DEFAULT_LEASE_TTL_MS)]
        ttl_ms: i64,
    },
    Release {
        session_id: String,
        owner: String,
        generation: u64,
    },
    Takeover {
        session_id: String,
        owner: String,
        #[arg(long, default_value_t = DEFAULT_LEASE_TTL_MS)]
        ttl_ms: i64,
    },
    Status {
        session_id: String,
    },
}

#[derive(Debug, Parser)]
enum TaskCommand {
    Status {
        task_id: String,
    },
    Stop {
        task_id: String,
        #[arg(long)]
        generation: Option<u64>,
    },
    Kill {
        task_ids: Vec<String>,
        #[arg(long)]
        generation: Option<u64>,
    },
    History,
}

fn request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("request-{}-{nanos}", std::process::id())
}

fn call(socket: &str, operation: &str, payload: Value) -> Result<Value, ProtocolErrorBody> {
    let mut stream = UnixStream::connect(socket).map_err(|error| ProtocolErrorBody {
        code: "transport".into(),
        message: error.to_string(),
    })?;
    verify_same_uid(&stream).map_err(|error| ProtocolErrorBody {
        code: "unauthorized_peer".into(),
        message: error.to_string(),
    })?;
    let id = request_id();
    let mutating = matches!(
        operation,
        "SESSION_CREATE"
            | "SESSION_INPUT"
            | "SESSION_DETACH"
            | "SESSION_CLOSE"
            | "SESSION_KILL"
            | "SERVER_KILL"
            | "SESSION_RESOURCE_UPDATE"
            | "TASK_STOP"
            | "TASK_KILL_BATCH"
            | "LEASE_CLAIM"
            | "LEASE_TOUCH"
            | "LEASE_RELEASE"
            | "LEASE_TAKEOVER"
    );
    let request = RequestEnvelope {
        version: PROTOCOL_VERSION,
        idempotency_key: mutating.then(|| id.clone()),
        request_id: id,
        operation: operation.into(),
        payload,
    };
    write_frame(&mut stream, &request).map_err(|error| ProtocolErrorBody {
        code: "transport".into(),
        message: error.to_string(),
    })?;
    let response: ResponseEnvelope =
        read_frame(&mut stream).map_err(|error| ProtocolErrorBody {
            code: "transport".into(),
            message: error.to_string(),
        })?;
    response.result
}

fn attach_error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
    }
}

fn attach_socket(socket: &str) -> String {
    format!("{socket}.attach")
}

fn terminal_size() -> (u16, u16) {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    let result = unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, size.as_mut_ptr()) };
    if result == 0 {
        let size = unsafe { size.assume_init() };
        if size.ws_row > 0 && size.ws_col > 0 {
            return (size.ws_row, size.ws_col);
        }
    }
    (24, 80)
}

struct RawTerminal {
    original: Option<libc::termios>,
}

impl RawTerminal {
    fn enter(enabled: bool) -> Result<Self, ProtocolErrorBody> {
        if !enabled {
            return Ok(Self { original: None });
        }
        let mut current = std::mem::MaybeUninit::<libc::termios>::zeroed();
        let result = unsafe { libc::tcgetattr(libc::STDIN_FILENO, current.as_mut_ptr()) };
        if result != 0 {
            return Err(attach_error(
                "transport",
                format!(
                    "failed to read terminal mode: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        let original = unsafe { current.assume_init() };
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) } != 0 {
            return Err(attach_error(
                "transport",
                format!(
                    "failed to enter raw terminal mode: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        Ok(Self {
            original: Some(original),
        })
    }

    fn reset_modes(&self) {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(TERMINAL_RESET_SEQUENCE);
        let _ = stdout.flush();
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        if let Some(original) = self.original {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &original);
            }
        }
    }
}

fn render_snapshot(snapshot: &attach::TerminalSnapshot) -> std::io::Result<()> {
    let mut output = std::io::stdout().lock();
    let alternate = if snapshot.alternate_screen {
        b"\x1b[?1049h".as_slice()
    } else {
        b"\x1b[?1049l".as_slice()
    };
    output.write_all(alternate)?;
    output.write_all(b"\x1b[2J\x1b[H")?;
    for (row, line) in snapshot
        .screen_text
        .split('\n')
        .take(usize::from(snapshot.rows))
        .enumerate()
    {
        write!(output, "\x1b[{};1H", row + 1)?;
        output.write_all(line.trim_end().as_bytes())?;
    }
    write!(
        output,
        "\x1b[{};{}H",
        u32::from(snapshot.cursor_row) + 1,
        u32::from(snapshot.cursor_column) + 1
    )?;
    output.write_all(if snapshot.cursor_visible {
        b"\x1b[?25h".as_slice()
    } else {
        b"\x1b[?25l".as_slice()
    })?;
    output.write_all(if snapshot.application_cursor {
        b"\x1b[?1h".as_slice()
    } else {
        b"\x1b[?1l".as_slice()
    })?;
    output.write_all(if snapshot.application_keypad {
        b"\x1b=".as_slice()
    } else {
        b"\x1b>".as_slice()
    })?;
    output.write_all(if snapshot.bracketed_paste {
        b"\x1b[?2004h".as_slice()
    } else {
        b"\x1b[?2004l".as_slice()
    })?;
    output.write_all(mouse_mode(snapshot))?;
    output.write_all(cursor_style(snapshot.cursor_style))?;
    output.flush()
}

fn mouse_mode(snapshot: &attach::TerminalSnapshot) -> &'static [u8] {
    let enable = match snapshot.mouse_protocol_mode {
        attach::MouseProtocolMode::None => b"\x1b[?1000l\x1b[?1002l\x1b[?1003l".as_slice(),
        attach::MouseProtocolMode::Press => b"\x1b[?1000h\x1b[?1002l\x1b[?1003l".as_slice(),
        attach::MouseProtocolMode::PressRelease => b"\x1b[?1000h\x1b[?1002l\x1b[?1003l".as_slice(),
        attach::MouseProtocolMode::ButtonMotion => b"\x1b[?1000l\x1b[?1002h\x1b[?1003l".as_slice(),
        attach::MouseProtocolMode::AnyMotion => b"\x1b[?1000l\x1b[?1002l\x1b[?1003h".as_slice(),
    };
    match snapshot.mouse_protocol_encoding {
        attach::MouseProtocolEncoding::Sgr => {
            if snapshot.mouse_protocol_mode == attach::MouseProtocolMode::None {
                b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l".as_slice()
            } else if matches!(
                snapshot.mouse_protocol_mode,
                attach::MouseProtocolMode::Press | attach::MouseProtocolMode::PressRelease
            ) {
                b"\x1b[?1000h\x1b[?1006h".as_slice()
            } else if snapshot.mouse_protocol_mode == attach::MouseProtocolMode::ButtonMotion {
                b"\x1b[?1002h\x1b[?1006h".as_slice()
            } else {
                b"\x1b[?1003h\x1b[?1006h".as_slice()
            }
        }
        _ => enable,
    }
}

fn cursor_style(style: attach::CursorStyle) -> &'static [u8] {
    match style {
        attach::CursorStyle::Default => b"\x1b[0 q".as_slice(),
        attach::CursorStyle::BlinkBlock => b"\x1b[1 q".as_slice(),
        attach::CursorStyle::SteadyBlock => b"\x1b[2 q".as_slice(),
        attach::CursorStyle::BlinkUnderline => b"\x1b[3 q".as_slice(),
        attach::CursorStyle::SteadyUnderline => b"\x1b[4 q".as_slice(),
        attach::CursorStyle::BlinkVerticalBar => b"\x1b[5 q".as_slice(),
        attach::CursorStyle::SteadyVerticalBar => b"\x1b[6 q".as_slice(),
    }
}

fn spawn_attach_input(
    writer: Arc<std::sync::Mutex<UnixStream>>,
    detach_requested: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0_u8; 8192];
        loop {
            let count = match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => count,
            };
            let detach_at = buffer[..count].iter().position(|byte| *byte == 0x1d);
            let input_end = detach_at.unwrap_or(count);
            if input_end > 0 {
                let mut stream = writer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if attach::write_payload_frame(
                    &mut *stream,
                    attach::FrameType::Input,
                    &buffer[..input_end],
                )
                .is_err()
                {
                    return;
                }
            }
            if detach_at.is_some() {
                let mut stream = writer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let detach = attach::Detach {
                    protocol_version: ATTACH_PROTOCOL_VERSION,
                    hold_ttl_ms: 0,
                };
                let _ = attach::write_json_frame(&mut *stream, attach::FrameType::Detach, &detach);
                let _ = stream.shutdown(std::net::Shutdown::Write);
                detach_requested.store(true, Ordering::SeqCst);
                return;
            }
        }
    })
}

fn spawn_resize_poll(
    writer: Arc<std::sync::Mutex<UnixStream>>,
    stop: Arc<AtomicBool>,
    initial: (u16, u16),
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut previous = initial;
        while !stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(200));
            let current = terminal_size();
            if current == previous {
                continue;
            }
            previous = current;
            let resize = attach::Resize {
                protocol_version: ATTACH_PROTOCOL_VERSION,
                rows: current.0,
                columns: current.1,
            };
            let mut stream = writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if attach::write_json_frame(&mut *stream, attach::FrameType::Resize, &resize).is_err() {
                return;
            }
        }
    })
}

fn run_control_input(
    control_socket: &str,
    session_id: &str,
    owner: &str,
    data: &str,
) -> Result<Value, ProtocolErrorBody> {
    let attach_path = attach_socket(control_socket);
    let mut stream = UnixStream::connect(&attach_path)
        .map_err(|failure| attach_error("transport", format!("{attach_path}: {failure}")))?;
    verify_same_uid(&stream)
        .map_err(|failure| attach_error("unauthorized_peer", failure.to_string()))?;
    let open = attach::AttachOpen::versioned(session_id, owner, 24, 80);
    attach::write_json_frame(&mut stream, attach::FrameType::AttachOpen, &open)
        .map_err(|failure| attach_error("transport", failure.to_string()))?;
    loop {
        let frame = attach::read_frame(&mut stream)
            .map_err(|failure| attach_error("transport", failure.to_string()))?;
        match frame.frame_type {
            attach::FrameType::AttachAccepted => {}
            attach::FrameType::Snapshot => break,
            attach::FrameType::Error => {
                let failure = attach::decode_json::<attach::AttachError>(&frame.payload).map_err(
                    |failure| attach_error("worker_protocol_error", failure.to_string()),
                )?;
                return Err(attach_error(&failure.code, failure.message));
            }
            _ => {
                return Err(attach_error(
                    "worker_protocol_error",
                    format!(
                        "expected ATTACH_ACCEPTED and SNAPSHOT, received {}",
                        frame.frame_type.name()
                    ),
                ));
            }
        }
    }
    let result = call(
        control_socket,
        "SESSION_INPUT",
        json!({"session_id": session_id, "owner": owner, "data": data}),
    );
    let detach = attach::Detach {
        protocol_version: ATTACH_PROTOCOL_VERSION,
        hold_ttl_ms: 0,
    };
    let _ = attach::write_json_frame(&mut stream, attach::FrameType::Detach, &detach);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    result
}

fn run_attach(
    control_socket: &str,
    session_id: &str,
    owner: &str,
) -> Result<Value, ProtocolErrorBody> {
    let attach_path = attach_socket(control_socket);
    let mut reader = UnixStream::connect(&attach_path)
        .map_err(|failure| attach_error("transport", format!("{attach_path}: {failure}")))?;
    verify_same_uid(&reader)
        .map_err(|failure| attach_error("unauthorized_peer", failure.to_string()))?;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let (rows, columns) = terminal_size();
    let open = attach::AttachOpen::versioned(session_id, owner, rows, columns);
    attach::write_json_frame(&mut reader, attach::FrameType::AttachOpen, &open)
        .map_err(|failure| attach_error("transport", failure.to_string()))?;
    let writer =
        Arc::new(std::sync::Mutex::new(reader.try_clone().map_err(
            |failure| attach_error("transport", failure.to_string()),
        )?));
    let terminal = RawTerminal::enter(interactive)?;
    let stop = Arc::new(AtomicBool::new(false));
    let detach_requested = Arc::new(AtomicBool::new(false));
    let _input_thread = spawn_attach_input(Arc::clone(&writer), Arc::clone(&detach_requested));
    let resize_thread = interactive
        .then(|| spawn_resize_poll(Arc::clone(&writer), Arc::clone(&stop), (rows, columns)));

    let result = loop {
        let frame = match attach::read_frame(&mut reader) {
            Ok(frame) => frame,
            Err(_) if detach_requested.load(Ordering::SeqCst) => break Ok(Value::Null),
            Err(failure) => break Err(attach_error("transport", failure.to_string())),
        };
        match frame.frame_type {
            attach::FrameType::AttachAccepted => {
                if let Err(failure) = attach::decode_json::<attach::AttachAccepted>(&frame.payload)
                {
                    break Err(attach_error("worker_protocol_error", failure.to_string()));
                }
            }
            attach::FrameType::Snapshot => {
                let snapshot = match attach::decode_json::<attach::TerminalSnapshot>(&frame.payload)
                {
                    Ok(snapshot) => snapshot,
                    Err(failure) => {
                        break Err(attach_error("worker_protocol_error", failure.to_string()));
                    }
                };
                if let Err(failure) = render_snapshot(&snapshot) {
                    break Err(attach_error("transport", failure.to_string()));
                }
                let ack = attach::Ack {
                    protocol_version: ATTACH_PROTOCOL_VERSION,
                    stream_offset: snapshot.next_stream_offset,
                };
                let mut stream = writer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Err(failure) =
                    attach::write_json_frame(&mut *stream, attach::FrameType::Ack, &ack)
                {
                    break Err(attach_error("transport", failure.to_string()));
                }
            }
            attach::FrameType::Output => {
                let Ok((_, data)) = attach::decode_output(&frame.payload) else {
                    break Err(attach_error(
                        "worker_protocol_error",
                        "OUTPUT frame has no stream offset",
                    ));
                };
                let mut stdout = std::io::stdout().lock();
                if let Err(failure) = stdout.write_all(data).and_then(|()| stdout.flush()) {
                    break Err(attach_error("transport", failure.to_string()));
                }
            }
            attach::FrameType::ProcessExited => {
                let exited = match attach::decode_json::<attach::ProcessExited>(&frame.payload) {
                    Ok(exited) => exited,
                    Err(failure) => {
                        break Err(attach_error("worker_protocol_error", failure.to_string()));
                    }
                };
                if interactive {
                    let mut stdout = std::io::stdout().lock();
                    let _ = writeln!(
                        stdout,
                        "\r\n[process exited code={} signal={}]",
                        exited
                            .exit_code
                            .map_or_else(|| "none".to_string(), |code| code.to_string()),
                        exited.exit_signal.as_deref().unwrap_or("none")
                    );
                }
                break Ok(Value::Null);
            }
            attach::FrameType::OwnershipRevoked => {
                let revoked = attach::decode_json::<attach::OwnershipRevoked>(&frame.payload);
                break Err(attach_error(
                    "not_owner",
                    revoked.map_or_else(
                        |failure| failure.to_string(),
                        |value| format!("attachment authority revoked: {}", value.reason),
                    ),
                ));
            }
            attach::FrameType::Error => {
                let failure = attach::decode_json::<attach::AttachError>(&frame.payload);
                break Err(failure.map_or_else(
                    |failure| attach_error("worker_protocol_error", failure.to_string()),
                    |failure| attach_error(&failure.code, failure.message),
                ));
            }
            attach::FrameType::Resized | attach::FrameType::Ack | attach::FrameType::Detach => {}
            _ => {
                break Err(attach_error(
                    "unknown_frame_type",
                    format!("unexpected server frame {}", frame.frame_type.name()),
                ));
            }
        }
    };

    stop.store(true, Ordering::SeqCst);
    terminal.reset_modes();
    drop(terminal);
    drop(writer);
    let _ = reader.shutdown(std::net::Shutdown::Both);
    if let Some(resize_thread) = resize_thread {
        let _ = resize_thread.join();
    }
    result
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let result = match arguments.command {
        None => Ok(json!({
            "name": "qwe",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": PROTOCOL_VERSION
        })),
        Some(Command::Status) => call(&arguments.socket, "CONTROL_STATUS", json!({})),
        Some(Command::Ping) => call(&arguments.socket, "PING", json!({})),
        Some(Command::Session { command }) => match command {
            SessionCommand::Create {
                id,
                program,
                credential,
                resources,
                args,
            } => {
                let mut payload = json!({
                    "session_id": id,
                    "program": program,
                    "args": args
                });
                if let Some(credential) = credential {
                    payload["credential"] = json!(credential);
                }
                if let Some(policy) = resources.policy_json(false) {
                    payload["resource_policy"] = policy;
                }
                call(&arguments.socket, "SESSION_CREATE", payload)
            }
            SessionCommand::Limits {
                id,
                generation,
                clear,
                resources,
            } => {
                let Some(policy) = resources.policy_json(clear) else {
                    eprintln!("session limits requires at least one limit or --clear");
                    return ExitCode::from(2);
                };
                call(
                    &arguments.socket,
                    "SESSION_RESOURCE_UPDATE",
                    json!({
                        "session_id": id,
                        "owner": arguments.owner,
                        "generation": generation,
                        "resource_policy": policy
                    }),
                )
            }
            SessionCommand::Status { id } => call(
                &arguments.socket,
                "SESSION_STATUS",
                json!({"session_id": id}),
            ),
            SessionCommand::Attach { id } => run_attach(&arguments.socket, &id, &arguments.owner),
            SessionCommand::Snapshot { id } => call(
                &arguments.socket,
                "SESSION_ATTACH",
                json!({"session_id": id}),
            ),
            SessionCommand::Input { id, data } => {
                run_control_input(&arguments.socket, &id, &arguments.owner, &data)
            }
            SessionCommand::Detach { id } => call(
                &arguments.socket,
                "SESSION_DETACH",
                json!({"session_id": id}),
            ),
            SessionCommand::Close { id, generation } => call(
                &arguments.socket,
                "SESSION_CLOSE",
                json!({
                    "session_id": id,
                    "owner": arguments.owner,
                    "generation": generation
                }),
            ),
        },
        Some(Command::Lease { command }) => match command {
            LeaseCommand::Claim {
                session_id,
                owner,
                ttl_ms,
            } => call(
                &arguments.socket,
                "LEASE_CLAIM",
                json!({"session_id": session_id, "owner": owner, "ttl_ms": ttl_ms}),
            ),
            LeaseCommand::Touch {
                session_id,
                owner,
                generation,
                ttl_ms,
            } => call(
                &arguments.socket,
                "LEASE_TOUCH",
                json!({
                    "session_id": session_id,
                    "owner": owner,
                    "generation": generation,
                    "ttl_ms": ttl_ms
                }),
            ),
            LeaseCommand::Release {
                session_id,
                owner,
                generation,
            } => call(
                &arguments.socket,
                "LEASE_RELEASE",
                json!({
                    "session_id": session_id,
                    "owner": owner,
                    "generation": generation
                }),
            ),
            LeaseCommand::Takeover {
                session_id,
                owner,
                ttl_ms,
            } => call(
                &arguments.socket,
                "LEASE_TAKEOVER",
                json!({"session_id": session_id, "owner": owner, "ttl_ms": ttl_ms}),
            ),
            LeaseCommand::Status { session_id } => call(
                &arguments.socket,
                "LEASE_STATUS",
                json!({"session_id": session_id}),
            ),
        },
        Some(Command::Task { command }) => match command {
            TaskCommand::Status { task_id } => call(
                &arguments.socket,
                "TASK_STATUS",
                json!({"task_id": task_id}),
            ),
            TaskCommand::Stop {
                task_id,
                generation,
            } => call(
                &arguments.socket,
                "TASK_STOP",
                json!({
                    "task_id": task_id,
                    "owner": arguments.owner,
                    "generation": generation
                }),
            ),
            TaskCommand::Kill {
                task_ids,
                generation,
            } => call(
                &arguments.socket,
                "TASK_KILL_BATCH",
                json!({
                    "task_ids": task_ids,
                    "owner": arguments.owner,
                    "generation": generation
                }),
            ),
            TaskCommand::History => call(&arguments.socket, "TASK_HISTORY", json!({})),
        },
        Some(Command::KillSession { id, generation }) => call(
            &arguments.socket,
            "SESSION_KILL",
            json!({
                "session_id": id,
                "owner": arguments.owner,
                "generation": generation
            }),
        ),
        Some(Command::KillServer { confirm }) => call(
            &arguments.socket,
            "SERVER_KILL",
            json!({"confirm": confirm}),
        ),
    };
    match result {
        Ok(value) => {
            if !value.is_null() {
                println!("{value}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TERMINAL_RESET_SEQUENCE;

    #[test]
    fn terminal_reset_disables_kitty_and_modified_key_protocols() {
        assert!(
            TERMINAL_RESET_SEQUENCE
                .windows(5)
                .any(|value| value == b"\x1b[<1u")
        );
        assert!(
            TERMINAL_RESET_SEQUENCE
                .windows(5)
                .any(|value| value == b"\x1b[=0u")
        );
        assert!(
            TERMINAL_RESET_SEQUENCE
                .windows(7)
                .any(|value| value == b"\x1b[>4;0m")
        );
        assert!(
            TERMINAL_RESET_SEQUENCE
                .windows(8)
                .any(|value| value == b"\x1b[?2004l")
        );
        assert!(
            TERMINAL_RESET_SEQUENCE
                .windows(8)
                .any(|value| value == b"\x1b[?1049l")
        );
    }
}
