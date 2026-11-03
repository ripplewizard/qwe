use clap::Parser;
use qwe_protocol::{
    PROTOCOL_VERSION, ProtocolErrorBody, RequestEnvelope, ResponseEnvelope, read_frame,
    verify_same_uid, write_frame,
};
use serde_json::{Value, json};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
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
            SessionCommand::Attach { id } => call(
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
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            ExitCode::FAILURE
        }
    }
}
