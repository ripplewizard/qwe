use clap::Parser;
use qwe_protocol::{
    PROTOCOL_VERSION, ProtocolErrorBody, RequestEnvelope, ResponseEnvelope, read_frame,
    verify_same_uid, write_frame,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    socket: String,
    #[arg(long)]
    host_socket: String,
}

fn error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
    }
}

fn proxy(
    mut client: UnixStream,
    host_socket: &str,
    epoch: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    verify_same_uid(&client)?;
    let mut request: RequestEnvelope = read_frame(&mut client)?;
    request.host_epoch = Some(epoch);
    let mut host = match UnixStream::connect(host_socket) {
        Ok(host) => host,
        Err(cause) => {
            let response = ResponseEnvelope {
                version: PROTOCOL_VERSION,
                request_id: request.request_id.clone(),
                result: Err(error("host_unavailable", cause.to_string())),
            };
            write_frame(&mut client, &response)?;
            return Ok(());
        }
    };
    verify_same_uid(&host)?;
    write_frame(&mut host, &request)?;
    let response: ResponseEnvelope = read_frame(&mut host)?;
    write_frame(&mut client, &response)?;
    Ok(())
}

fn register(host_socket: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let mut host = UnixStream::connect(host_socket)?;
    let request = RequestEnvelope {
        version: PROTOCOL_VERSION,
        request_id: format!("qwed-register-{}", std::process::id()),
        idempotency_key: None,
        host_epoch: None,
        operation: "HOST_REGISTER".into(),
        payload: serde_json::json!({}),
    };
    write_frame(&mut host, &request)?;
    let response: ResponseEnvelope = read_frame(&mut host)?;
    match response.result {
        Ok(value) => value
            .get("epoch")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "host registration response has no epoch".into()),
        Err(error) => Err(format!("{}: {}", error.code, error.message).into()),
    }
}

fn existing_instance(socket: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let request = RequestEnvelope {
        version: PROTOCOL_VERSION,
        request_id: format!("qwed-singleton-{}", std::process::id()),
        idempotency_key: None,
        host_epoch: Some(1),
        operation: "PING".into(),
        payload: serde_json::json!({}),
    };
    if write_frame(&mut stream, &request).is_err() {
        return false;
    }
    read_frame::<_, ResponseEnvelope>(&mut stream).is_ok()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if existing_instance(&args.socket) {
        return Err(format!("qwed is already running at {}", args.socket).into());
    }
    let _ = fs::remove_file(&args.socket);
    let listener = UnixListener::bind(&args.socket)?;
    fs::set_permissions(&args.socket, fs::Permissions::from_mode(0o600))?;
    let epoch = register(&args.host_socket)?;
    for client in listener.incoming() {
        if let Err(error) = proxy(client?, &args.host_socket, epoch) {
            eprintln!("proxy failed: {error}");
        }
    }
    Ok(())
}
