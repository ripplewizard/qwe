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
    let request = RequestEnvelope {
        version: PROTOCOL_VERSION,
        request_id: request_id(),
        idempotency_key: None,
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
        Some(Command::Status | Command::Ping) => call(&arguments.socket, "PING", json!({})),
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
