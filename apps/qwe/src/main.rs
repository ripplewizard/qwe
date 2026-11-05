use clap::Parser;
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

#[derive(Debug, Parser)]
#[command(version, about = "Quick Window Environment CLI")]
struct Arguments {
    #[arg(long, global = true, default_value = "/tmp/qwe-client.sock")]
    socket: String,
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
}

#[derive(Debug, Parser)]
enum SessionCommand {
    Create {
        id: String,
        #[arg(default_value = "sh")]
        program: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
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
    },
}

#[derive(Debug, Parser)]
enum LeaseCommand {
    Claim {
        session_id: String,
        owner: String,
        #[arg(long, default_value_t = 30_000)]
        ttl_ms: i64,
    },
    Touch {
        session_id: String,
        owner: String,
        generation: u64,
        #[arg(long, default_value_t = 30_000)]
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
        #[arg(long, default_value_t = 30_000)]
        ttl_ms: i64,
    },
    Status {
        session_id: String,
    },
}

#[derive(Debug, Parser)]
enum TaskCommand {
    Start {
        id: String,
        #[arg(default_value = "sh")]
        program: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    Status {
        task_id: String,
    },
    Stop {
        task_id: String,
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
            | "TASK_START"
            | "TASK_STOP"
            | "LEASE_CLAIM"
            | "LEASE_TOUCH"
            | "LEASE_RELEASE"
            | "LEASE_TAKEOVER"
    );
    let request = RequestEnvelope {
        version: PROTOCOL_VERSION,
        idempotency_key: mutating.then(|| id.clone()),
        request_id: id,
        host_epoch: None,
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
        let _ = stdout.write_all(
            b"\x1b[0m\x1b[?25h\x1b[?1l\x1b>\x1b[?2004l\x1b[?1049l\
              \x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l",
        );
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

fn run_attach(control_socket: &str, session_id: &str) -> Result<Value, ProtocolErrorBody> {
    let attach_path = attach_socket(control_socket);
    let mut reader = UnixStream::connect(&attach_path)
        .map_err(|failure| attach_error("transport", format!("{attach_path}: {failure}")))?;
    verify_same_uid(&reader)
        .map_err(|failure| attach_error("unauthorized_peer", failure.to_string()))?;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let (rows, columns) = terminal_size();
    let owner = format!("cli-{}", std::process::id());
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
            SessionCommand::Create { id, program, args } => call(
                &arguments.socket,
                "SESSION_CREATE",
                json!({"session_id": id, "program": program, "args": args}),
            ),
            SessionCommand::Status { id } => call(
                &arguments.socket,
                "SESSION_STATUS",
                json!({"session_id": id}),
            ),
            SessionCommand::Attach { id } => run_attach(&arguments.socket, &id),
            SessionCommand::Snapshot { id } => call(
                &arguments.socket,
                "SESSION_ATTACH",
                json!({"session_id": id}),
            ),
            SessionCommand::Input { id, data } => call(
                &arguments.socket,
                "SESSION_INPUT",
                json!({"session_id": id, "data": data}),
            ),
            SessionCommand::Detach { id } => call(
                &arguments.socket,
                "SESSION_DETACH",
                json!({"session_id": id}),
            ),
            SessionCommand::Close { id } => call(
                &arguments.socket,
                "SESSION_CLOSE",
                json!({"session_id": id}),
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
            TaskCommand::Start { id, program, args } => call(
                &arguments.socket,
                "TASK_START",
                json!({"session_id": id, "program": program, "args": args}),
            ),
            TaskCommand::Status { task_id } => call(
                &arguments.socket,
                "TASK_STATUS",
                json!({"task_id": task_id}),
            ),
            TaskCommand::Stop { task_id } => {
                call(&arguments.socket, "TASK_STOP", json!({"task_id": task_id}))
            }
            TaskCommand::History => call(&arguments.socket, "TASK_HISTORY", json!({})),
        },
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
