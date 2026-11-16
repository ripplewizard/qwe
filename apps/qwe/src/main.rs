use clap::{Args, Parser};
use qwe_core::DEFAULT_LEASE_TTL_MS;
use qwe_protocol::{
    ATTACH_PROTOCOL_VERSION, AuthContext, PROTOCOL_VERSION, ProtocolErrorBody, RequestEnvelope,
    ResponseEnvelope, attach, read_frame, verify_same_uid, write_frame,
};
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::{IsTerminal, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

mod compositor;

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
    #[arg(long, global = true)]
    capability_file: Option<PathBuf>,
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
    Pane {
        #[command(subcommand)]
        command: PaneCommand,
    },
    Lease {
        #[command(subcommand)]
        command: LeaseCommand,
    },
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    Security {
        #[command(subcommand)]
        command: SecurityCommand,
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
        resources: Box<ResourceArgs>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
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
    Compose {
        id: String,
    },
    Layout {
        id: String,
        #[arg(long)]
        rows: Option<u16>,
        #[arg(long)]
        columns: Option<u16>,
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

#[derive(Debug, Parser)]
enum PaneCommand {
    Create {
        session_id: String,
        #[arg(default_value = "sh")]
        program: String,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value = "normal")]
        kind: String,
        #[arg(long)]
        generation: Option<u64>,
        #[arg(long)]
        credential: Option<String>,
        #[command(flatten)]
        resources: Box<ResourceArgs>,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    Status {
        pane_id: String,
    },
    Focus {
        pane_id: String,
        #[arg(long)]
        generation: Option<u64>,
    },
    Reorder {
        pane_ids: Vec<String>,
        #[arg(long)]
        generation: Option<u64>,
    },
    Move {
        pane_id: String,
        #[arg(long)]
        x: u16,
        #[arg(long)]
        y: u16,
        #[arg(long)]
        rows: u16,
        #[arg(long)]
        columns: u16,
        #[arg(long)]
        generation: Option<u64>,
    },
    Raise {
        pane_id: String,
        #[arg(long)]
        generation: Option<u64>,
    },
    Snapshot {
        pane_id: String,
    },
    Close {
        pane_id: String,
        #[arg(long)]
        generation: Option<u64>,
    },
    Kill {
        pane_id: String,
        #[arg(long)]
        generation: Option<u64>,
    },
}

#[derive(Debug, Args)]
struct ResourceArgs {
    #[arg(long)]
    profile: Option<String>,
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

#[derive(Debug, Parser)]
enum AgentCommand {
    Exec {
        pane_id: String,
        program: String,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        deadline_ms: Option<i64>,
        #[arg(long)]
        credential: Option<String>,
        #[arg(long)]
        pty: bool,
        #[arg(long, default_value_t = 24)]
        rows: u16,
        #[arg(long, default_value_t = 80)]
        columns: u16,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Status {
        command_id: String,
    },
    Events {
        command_id: String,
        #[arg(long, default_value_t = 0)]
        cursor: u64,
        #[arg(long, default_value_t = 256)]
        limit: u64,
    },
    Wait {
        command_id: String,
        #[arg(long, default_value_t = qwe_core::AGENT_MAX_WAIT_MS)]
        timeout_ms: u64,
    },
    Terminate {
        command_id: String,
    },
    Signal {
        command_id: String,
        #[arg(long)]
        signal: String,
    },
    Observe {
        command_id: String,
        #[arg(long, default_value_t = 0)]
        cursor: u64,
        #[arg(long, default_value_t = qwe_core::AGENT_MAX_WAIT_MS)]
        timeout_ms: u64,
    },
}

#[derive(Debug, Parser)]
enum SecurityCommand {
    Bootstrap,
    PrincipalCreate {
        parent_id: String,
        #[arg(long)]
        high_trust: bool,
    },
    CapabilityIssue {
        principal_id: String,
        operation: String,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        pane_id: Option<String>,
        #[arg(long)]
        command_id: Option<String>,
        #[arg(long, default_value_t = qwe_core::DEFAULT_CAPABILITY_TTL_MS)]
        ttl_ms: i64,
        #[arg(long)]
        sandbox_profile: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Revoke {
        principal_id: String,
        #[arg(long)]
        severity: Option<String>,
        #[arg(long, default_value_t = qwe_core::DEFAULT_FREEZE_GRACE_MS)]
        grace_ms: i64,
        #[arg(long)]
        hold: bool,
    },
}

fn request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("request-{}-{nanos}", std::process::id())
}

fn call(socket: &str, operation: &str, payload: Value) -> Result<Value, ProtocolErrorBody> {
    call_with_auth(socket, operation, payload, None)
}

fn call_with_auth(
    socket: &str,
    operation: &str,
    payload: Value,
    auth_context: Option<AuthContext>,
) -> Result<Value, ProtocolErrorBody> {
    let mut stream = UnixStream::connect(socket).map_err(|error| ProtocolErrorBody {
        code: "transport".into(),
        message: error.to_string(),
        details: None,
    })?;
    verify_same_uid(&stream).map_err(|error| ProtocolErrorBody {
        code: "unauthorized_peer".into(),
        message: error.to_string(),
        details: None,
    })?;
    let id = request_id();
    let mutating = matches!(
        operation,
        "SESSION_CREATE"
            | "AGENT_EXEC"
            | "AGENT_COMMAND_TERMINATE"
            | "AGENT_COMMAND_SIGNAL"
            | "SECURITY_PRINCIPAL_CREATE"
            | "SECURITY_PRINCIPAL_REVOKE"
            | "PANE_CREATE"
            | "PANE_FOCUS"
            | "PANE_REORDER"
            | "PANE_FLOATING_UPDATE"
            | "PANE_FLOATING_RAISE"
            | "PANE_CLOSE"
            | "PANE_KILL"
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
        auth_context,
    };
    write_frame(&mut stream, &request).map_err(|error| ProtocolErrorBody {
        code: "transport".into(),
        message: error.to_string(),
        details: None,
    })?;
    let response: ResponseEnvelope =
        read_frame(&mut stream).map_err(|error| ProtocolErrorBody {
            code: "transport".into(),
            message: error.to_string(),
            details: None,
        })?;
    response.result
}

fn read_capability_file(path: &Path) -> Result<AuthContext, ProtocolErrorBody> {
    let metadata = std::fs::metadata(path).map_err(|failure| ProtocolErrorBody {
        code: "capability_file_unavailable".into(),
        message: format!("failed to inspect {}: {failure}", path.display()),
        details: None,
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(ProtocolErrorBody {
            code: "capability_file_insecure".into(),
            message: format!("{} must be a private regular file", path.display()),
            details: None,
        });
    }
    let capability = std::fs::read_to_string(path).map_err(|failure| ProtocolErrorBody {
        code: "capability_file_unavailable".into(),
        message: format!("failed to read {}: {failure}", path.display()),
        details: None,
    })?;
    let capability = capability.trim();
    if capability.is_empty() {
        return Err(ProtocolErrorBody {
            code: "capability_file_unavailable".into(),
            message: format!("{} is empty", path.display()),
            details: None,
        });
    }
    Ok(AuthContext {
        capability: capability.to_string(),
    })
}

fn optional_capability(
    capability_file: Option<&Path>,
) -> Result<Option<AuthContext>, ProtocolErrorBody> {
    capability_file.map(read_capability_file).transpose()
}

fn management_capability(
    socket: &str,
    capability_file: Option<&Path>,
) -> Result<AuthContext, ProtocolErrorBody> {
    let path = capability_file
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(format!("{socket}.runtime/security.bootstrap")));
    read_capability_file(&path)
}

fn capability_scope_json(
    operation: &str,
    session_id: Option<String>,
    pane_id: Option<String>,
    command_id: Option<String>,
) -> Result<Value, ProtocolErrorBody> {
    if matches!(operation, "manage_principals") {
        if session_id.is_some() || pane_id.is_some() || command_id.is_some() {
            return Err(ProtocolErrorBody {
                code: "invalid_request".into(),
                message: "management capabilities use global scope".into(),
                details: None,
            });
        }
        return Ok(json!({"kind": "global"}));
    }
    if let Some(command_id) = command_id {
        let pane_id = pane_id.ok_or_else(|| ProtocolErrorBody {
            code: "invalid_request".into(),
            message: "--pane-id is required with --command-id".into(),
            details: None,
        })?;
        if session_id.is_some() {
            return Err(ProtocolErrorBody {
                code: "invalid_request".into(),
                message: "command scope cannot include --session-id".into(),
                details: None,
            });
        }
        return Ok(json!({
            "kind": "command",
            "pane_id": pane_id,
            "command_id": command_id
        }));
    }
    if let Some(pane_id) = pane_id {
        if session_id.is_some() {
            return Err(ProtocolErrorBody {
                code: "invalid_request".into(),
                message: "pane scope cannot include --session-id".into(),
                details: None,
            });
        }
        return Ok(json!({"kind": "pane", "pane_id": pane_id}));
    }
    if let Some(session_id) = session_id {
        return Ok(json!({"kind": "session", "session_id": session_id}));
    }
    Err(ProtocolErrorBody {
        code: "invalid_request".into(),
        message: "a capability scope is required".into(),
        details: None,
    })
}

fn write_capability_output(path: &Path, result: &mut Value) -> Result<(), ProtocolErrorBody> {
    let token = result
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolErrorBody {
            code: "protocol_error".into(),
            message: "capability response has no token".into(),
            details: None,
        })?
        .to_string();
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|failure| ProtocolErrorBody {
            code: "capability_output_failed".into(),
            message: format!("failed to create {}: {failure}", path.display()),
            details: None,
        })?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|failure| ProtocolErrorBody {
            code: "capability_output_failed".into(),
            message: format!("failed to secure {}: {failure}", path.display()),
            details: None,
        })?;
    file.write_all(token.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.flush())
        .map_err(|failure| ProtocolErrorBody {
            code: "capability_output_failed".into(),
            message: format!("failed to write {}: {failure}", path.display()),
            details: None,
        })?;
    if let Some(object) = result.as_object_mut() {
        object.remove("token");
    }
    Ok(())
}

fn run_agent_observe(
    socket: &str,
    auth_context: AuthContext,
    payload: Value,
) -> Result<Value, ProtocolErrorBody> {
    let mut stream = UnixStream::connect(socket).map_err(|failure| ProtocolErrorBody {
        code: "transport".into(),
        message: failure.to_string(),
        details: None,
    })?;
    verify_same_uid(&stream).map_err(|failure| ProtocolErrorBody {
        code: "unauthorized_peer".into(),
        message: failure.to_string(),
        details: None,
    })?;
    let request_id = request_id();
    let request = RequestEnvelope {
        version: PROTOCOL_VERSION,
        idempotency_key: None,
        request_id: request_id.clone(),
        operation: "AGENT_COMMAND_OBSERVE".into(),
        payload,
        auth_context: Some(auth_context),
    };
    write_frame(&mut stream, &request).map_err(|failure| ProtocolErrorBody {
        code: "transport".into(),
        message: failure.to_string(),
        details: None,
    })?;
    loop {
        let response: ResponseEnvelope =
            read_frame(&mut stream).map_err(|failure| ProtocolErrorBody {
                code: "transport".into(),
                message: failure.to_string(),
                details: None,
            })?;
        if response.request_id != request_id {
            return Err(ProtocolErrorBody {
                code: "protocol_error".into(),
                message: "observer response request_id mismatch".into(),
                details: None,
            });
        }
        let value = response.result?;
        match value.get("type").and_then(Value::as_str) {
            Some("end") => return Ok(Value::Null),
            Some("error") => {
                return Err(ProtocolErrorBody {
                    code: value
                        .get("code")
                        .and_then(Value::as_str)
                        .unwrap_or("observer_error")
                        .into(),
                    message: value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("observer failed")
                        .into(),
                    details: None,
                });
            }
            _ => println!("{value}"),
        }
    }
}

fn attach_error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
        details: None,
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
    input_auth_context: Option<AuthContext>,
) -> Result<Value, ProtocolErrorBody> {
    let attach_path = attach_socket(control_socket);
    let mut stream = UnixStream::connect(&attach_path)
        .map_err(|failure| attach_error("transport", format!("{attach_path}: {failure}")))?;
    verify_same_uid(&stream)
        .map_err(|failure| attach_error("unauthorized_peer", failure.to_string()))?;
    let mut open = attach::AttachOpen::versioned(session_id, owner, 24, 80);
    if let Some(auth_context) = input_auth_context.clone() {
        open = open.with_input_auth_context(auth_context);
    }
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
    let result = call_with_auth(
        control_socket,
        "SESSION_INPUT",
        json!({"session_id": session_id, "owner": owner, "data": data}),
        input_auth_context,
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
    input_auth_context: Option<AuthContext>,
) -> Result<Value, ProtocolErrorBody> {
    let attach_path = attach_socket(control_socket);
    let mut reader = UnixStream::connect(&attach_path)
        .map_err(|failure| attach_error("transport", format!("{attach_path}: {failure}")))?;
    verify_same_uid(&reader)
        .map_err(|failure| attach_error("unauthorized_peer", failure.to_string()))?;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let (rows, columns) = terminal_size();
    let mut open = attach::AttachOpen::versioned(session_id, owner, rows, columns);
    if let Some(auth_context) = input_auth_context {
        open = open.with_input_auth_context(auth_context);
    }
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
                let profile = resources.profile.clone();
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
                if let Some(profile) = profile {
                    payload["resource_profile"] = json!(profile);
                }
                call(&arguments.socket, "SESSION_CREATE", payload)
            }
            SessionCommand::Limits {
                id,
                generation,
                clear,
                resources,
            } => {
                let profile = resources.profile.clone();
                if clear && profile.is_some() {
                    eprintln!("session limits cannot combine --clear with --profile");
                    return ExitCode::from(2);
                }
                let policy = resources.policy_json(clear);
                if policy.is_none() && profile.is_none() {
                    eprintln!("session limits requires --profile, at least one limit, or --clear");
                    return ExitCode::from(2);
                }
                let mut payload = json!({
                    "session_id": id,
                    "owner": arguments.owner,
                    "generation": generation
                });
                if let Some(policy) = policy {
                    payload["resource_policy"] = policy;
                }
                if let Some(profile) = profile {
                    payload["resource_profile"] = json!(profile);
                }
                call(&arguments.socket, "SESSION_RESOURCE_UPDATE", payload)
            }
            SessionCommand::Status { id } => call(
                &arguments.socket,
                "SESSION_STATUS",
                json!({"session_id": id}),
            ),
            SessionCommand::Attach { id } => {
                let auth = match optional_capability(arguments.capability_file.as_deref()) {
                    Ok(auth) => auth,
                    Err(failure) => {
                        eprintln!("{}: {}", failure.code, failure.message);
                        return ExitCode::FAILURE;
                    }
                };
                run_attach(&arguments.socket, &id, &arguments.owner, auth)
            }
            SessionCommand::Compose { id } => {
                let auth = match optional_capability(arguments.capability_file.as_deref()) {
                    Ok(auth) => auth,
                    Err(failure) => {
                        eprintln!("{}: {}", failure.code, failure.message);
                        return ExitCode::FAILURE;
                    }
                };
                compositor::run(&arguments.socket, &id, &arguments.owner, auth)
            }
            SessionCommand::Layout { id, rows, columns } => {
                let (default_rows, default_columns) = terminal_size();
                call(
                    &arguments.socket,
                    "SESSION_LAYOUT",
                    json!({
                        "session_id": id,
                        "rows": rows.unwrap_or(default_rows),
                        "columns": columns.unwrap_or(default_columns)
                    }),
                )
            }
            SessionCommand::Snapshot { id } => call(
                &arguments.socket,
                "SESSION_ATTACH",
                json!({"session_id": id}),
            ),
            SessionCommand::Input { id, data } => {
                let auth = match optional_capability(arguments.capability_file.as_deref()) {
                    Ok(auth) => auth,
                    Err(failure) => {
                        eprintln!("{}: {}", failure.code, failure.message);
                        return ExitCode::FAILURE;
                    }
                };
                run_control_input(&arguments.socket, &id, &arguments.owner, &data, auth)
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
        Some(Command::Pane { command }) => match command {
            PaneCommand::Create {
                session_id,
                program,
                after,
                kind,
                generation,
                credential,
                resources,
                args,
            } => {
                let profile = resources.profile.clone();
                let mut payload = json!({
                    "session_id": session_id,
                    "program": program,
                    "args": args,
                    "owner": arguments.owner,
                    "generation": generation
                });
                if let Some(credential) = credential {
                    payload["credential"] = json!(credential);
                }
                if let Some(after) = after {
                    payload["after_pane_id"] = json!(after);
                }
                payload["pane_kind"] = json!(kind);
                if let Some(policy) = resources.policy_json(false) {
                    payload["resource_policy"] = policy;
                }
                if let Some(profile) = profile {
                    payload["resource_profile"] = json!(profile);
                }
                call(&arguments.socket, "PANE_CREATE", payload)
            }
            PaneCommand::Status { pane_id } => call(
                &arguments.socket,
                "PANE_STATUS",
                json!({"pane_id": pane_id}),
            ),
            PaneCommand::Focus {
                pane_id,
                generation,
            } => call(
                &arguments.socket,
                "PANE_FOCUS",
                json!({
                    "pane_id": pane_id,
                    "owner": arguments.owner,
                    "generation": generation
                }),
            ),
            PaneCommand::Reorder {
                pane_ids,
                generation,
            } => call(
                &arguments.socket,
                "PANE_REORDER",
                json!({
                    "pane_ids": pane_ids,
                    "owner": arguments.owner,
                    "generation": generation
                }),
            ),
            PaneCommand::Move {
                pane_id,
                x,
                y,
                rows,
                columns,
                generation,
            } => call(
                &arguments.socket,
                "PANE_FLOATING_UPDATE",
                json!({
                    "pane_id": pane_id,
                    "rect": {
                        "x": x,
                        "y": y,
                        "rows": rows,
                        "columns": columns
                    },
                    "owner": arguments.owner,
                    "generation": generation
                }),
            ),
            PaneCommand::Raise {
                pane_id,
                generation,
            } => call(
                &arguments.socket,
                "PANE_FLOATING_RAISE",
                json!({
                    "pane_id": pane_id,
                    "owner": arguments.owner,
                    "generation": generation
                }),
            ),
            PaneCommand::Snapshot { pane_id } => call(
                &arguments.socket,
                "PANE_ATTACH",
                json!({"pane_id": pane_id}),
            ),
            PaneCommand::Close {
                pane_id,
                generation,
            } => call(
                &arguments.socket,
                "PANE_CLOSE",
                json!({
                    "pane_id": pane_id,
                    "owner": arguments.owner,
                    "generation": generation
                }),
            ),
            PaneCommand::Kill {
                pane_id,
                generation,
            } => call(
                &arguments.socket,
                "PANE_KILL",
                json!({
                    "pane_id": pane_id,
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
        Some(Command::Agent { command }) => {
            let auth = match arguments.capability_file.as_deref() {
                Some(path) => match read_capability_file(path) {
                    Ok(auth) => auth,
                    Err(failure) => {
                        eprintln!("{}: {}", failure.code, failure.message);
                        return ExitCode::FAILURE;
                    }
                },
                None => {
                    eprintln!("capability_missing: agent commands require --capability-file");
                    return ExitCode::FAILURE;
                }
            };
            match command {
                AgentCommand::Exec {
                    pane_id,
                    program,
                    cwd,
                    deadline_ms,
                    credential,
                    pty,
                    rows,
                    columns,
                    args,
                } => {
                    let mut payload = json!({
                        "pane_id": pane_id,
                        "program": program,
                        "args": args,
                        "owner": arguments.owner
                    });
                    if let Some(cwd) = cwd {
                        payload["cwd"] = json!(cwd);
                    }
                    if let Some(deadline_ms) = deadline_ms {
                        payload["deadline_ms"] = json!(deadline_ms);
                    }
                    if let Some(credential) = credential {
                        payload["credential"] = json!(credential);
                    }
                    if pty {
                        payload["pty"] = json!(true);
                        payload["rows"] = json!(rows);
                        payload["columns"] = json!(columns);
                    } else if rows != 24 || columns != 80 {
                        eprintln!("invalid_request: --rows and --columns require --pty");
                        return ExitCode::FAILURE;
                    }
                    call_with_auth(&arguments.socket, "AGENT_EXEC", payload, Some(auth))
                }
                AgentCommand::Status { command_id } => call_with_auth(
                    &arguments.socket,
                    "AGENT_COMMAND_STATUS",
                    json!({"command_id": command_id}),
                    Some(auth),
                ),
                AgentCommand::Events {
                    command_id,
                    cursor,
                    limit,
                } => call_with_auth(
                    &arguments.socket,
                    "AGENT_COMMAND_EVENTS",
                    json!({"command_id": command_id, "cursor": cursor, "limit": limit}),
                    Some(auth),
                ),
                AgentCommand::Wait {
                    command_id,
                    timeout_ms,
                } => call_with_auth(
                    &arguments.socket,
                    "AGENT_COMMAND_WAIT_IDLE",
                    json!({"command_id": command_id, "timeout_ms": timeout_ms}),
                    Some(auth),
                ),
                AgentCommand::Terminate { command_id } => call_with_auth(
                    &arguments.socket,
                    "AGENT_COMMAND_TERMINATE",
                    json!({"command_id": command_id}),
                    Some(auth),
                ),
                AgentCommand::Signal { command_id, signal } => call_with_auth(
                    &arguments.socket,
                    "AGENT_COMMAND_SIGNAL",
                    json!({"command_id": command_id, "signal": signal}),
                    Some(auth),
                ),
                AgentCommand::Observe {
                    command_id,
                    cursor,
                    timeout_ms,
                } => run_agent_observe(
                    &arguments.socket,
                    auth,
                    json!({
                        "command_id": command_id,
                        "cursor": cursor,
                        "timeout_ms": timeout_ms
                    }),
                ),
            }
        }
        Some(Command::Security { command }) => {
            let auth = match management_capability(
                &arguments.socket,
                arguments.capability_file.as_deref(),
            ) {
                Ok(auth) => auth,
                Err(failure) => {
                    eprintln!("{}: {}", failure.code, failure.message);
                    return ExitCode::FAILURE;
                }
            };
            match command {
                SecurityCommand::Bootstrap => call_with_auth(
                    &arguments.socket,
                    "SECURITY_BOOTSTRAP",
                    json!({}),
                    Some(auth),
                ),
                SecurityCommand::PrincipalCreate {
                    parent_id,
                    high_trust,
                } => call_with_auth(
                    &arguments.socket,
                    "SECURITY_PRINCIPAL_CREATE",
                    json!({"parent_id": parent_id, "high_trust": high_trust}),
                    Some(auth),
                ),
                SecurityCommand::CapabilityIssue {
                    principal_id,
                    operation,
                    session_id,
                    pane_id,
                    command_id,
                    ttl_ms,
                    sandbox_profile,
                    output,
                } => {
                    let scope =
                        match capability_scope_json(&operation, session_id, pane_id, command_id) {
                            Ok(scope) => scope,
                            Err(failure) => {
                                eprintln!("{}: {}", failure.code, failure.message);
                                return ExitCode::FAILURE;
                            }
                        };
                    let mut payload = json!({
                        "principal_id": principal_id,
                        "operation": operation,
                        "scope": scope,
                        "ttl_ms": ttl_ms
                    });
                    if let Some(path) = sandbox_profile {
                        let profile = match std::fs::read(&path)
                            .map_err(|failure| failure.to_string())
                            .and_then(|contents| {
                                serde_json::from_slice::<Value>(&contents)
                                    .map_err(|failure| failure.to_string())
                            }) {
                            Ok(profile) if profile.is_object() => profile,
                            Ok(_) => {
                                eprintln!(
                                    "invalid_request: sandbox profile file must contain an object"
                                );
                                return ExitCode::FAILURE;
                            }
                            Err(failure) => {
                                eprintln!(
                                    "invalid_request: failed to read sandbox profile '{}': {failure}",
                                    path.display()
                                );
                                return ExitCode::FAILURE;
                            }
                        };
                        payload["sandbox_profile"] = profile;
                    }
                    match call_with_auth(
                        &arguments.socket,
                        "SECURITY_CAPABILITY_ISSUE",
                        payload,
                        Some(auth),
                    ) {
                        Ok(mut value) => {
                            if let Some(output) = output {
                                if let Err(failure) = write_capability_output(&output, &mut value) {
                                    Err(failure)
                                } else {
                                    Ok(value)
                                }
                            } else {
                                Ok(value)
                            }
                        }
                        Err(failure) => Err(failure),
                    }
                }
                SecurityCommand::Revoke {
                    principal_id,
                    severity,
                    grace_ms,
                    hold,
                } => call_with_auth(
                    &arguments.socket,
                    "SECURITY_PRINCIPAL_REVOKE",
                    json!({
                        "principal_id": principal_id,
                        "severity": severity,
                        "grace_ms": grace_ms,
                        "hold": hold
                    }),
                    Some(auth),
                ),
            }
        }
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
