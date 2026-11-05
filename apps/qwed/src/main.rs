use clap::Parser;
use qwe_core::{
    LeaseRecord, ProcessIdentity, SessionRecord, SessionState, TaskId, TaskRecord, TaskState,
};
use qwe_protocol::{
    ATTACH_PROTOCOL_VERSION, PROTOCOL_VERSION, ProtocolErrorBody, RequestEnvelope,
    ResponseEnvelope, attach, read_frame, validate_version, verify_same_uid, write_frame,
};
use qwe_storage::{LeaseError, StorageError, Store};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RECONCILE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Parser)]
struct Args {
    #[arg(long)]
    socket: String,
    #[arg(long)]
    host_socket: String,
    #[arg(long)]
    store: Option<String>,
}

struct Context {
    host_socket: String,
    epoch: AtomicU64,
    store: Mutex<Store>,
    reconciliation: Mutex<Value>,
}

#[derive(Default)]
struct Reconciliation {
    live_sessions: usize,
    orphaned_sessions: u64,
    lost_sessions: u64,
    exited_tasks: u64,
    lost_tasks: u64,
}

fn error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn request_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

fn attach_socket(socket: &str) -> String {
    format!("{socket}.attach")
}

fn call_host(
    host_socket: &str,
    epoch: Option<u64>,
    operation: &str,
    payload: Value,
) -> Result<Value, ProtocolErrorBody> {
    let mut host = UnixStream::connect(host_socket)
        .map_err(|failure| error("host_unavailable", format!("{host_socket}: {failure}")))?;
    verify_same_uid(&host).map_err(|failure| error("unauthorized_peer", failure.to_string()))?;
    let request = RequestEnvelope {
        version: PROTOCOL_VERSION,
        request_id: request_id("qwed-host"),
        idempotency_key: None,
        host_epoch: epoch,
        operation: operation.into(),
        payload,
    };
    write_frame(&mut host, &request).map_err(|failure| error("transport", failure.to_string()))?;
    let response: ResponseEnvelope =
        read_frame(&mut host).map_err(|failure| error("transport", failure.to_string()))?;
    validate_version(response.version)
        .map_err(|failure| error("incompatible_version", failure.to_string()))?;
    response.result
}

fn register(host_socket: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let value = call_host(host_socket, None, "HOST_REGISTER", json!({}))
        .map_err(|failure| format!("{}: {}", failure.code, failure.message))?;
    value
        .get("epoch")
        .and_then(Value::as_u64)
        .ok_or_else(|| "host registration response has no epoch".into())
}

fn host_call(
    context: &Context,
    operation: &str,
    payload: Value,
) -> Result<Value, ProtocolErrorBody> {
    let epoch = context.epoch.load(Ordering::SeqCst);
    match call_host(
        &context.host_socket,
        Some(epoch),
        operation,
        payload.clone(),
    ) {
        Err(failure) if failure.code == "stale_epoch" => {
            let epoch = register(&context.host_socket)
                .map_err(|failure| error("host_registration_failed", failure.to_string()))?;
            context.epoch.store(epoch, Ordering::SeqCst);
            call_host(&context.host_socket, Some(epoch), operation, payload)
        }
        result => result,
    }
}

fn storage_error(failure: StorageError) -> ProtocolErrorBody {
    match failure {
        StorageError::Lease(cause) => {
            let code = match cause {
                LeaseError::AlreadyHeld => "lease_held",
                LeaseError::Expired => "lease_expired",
                LeaseError::NotOwner => "not_owner",
                LeaseError::NotFound => "not_found",
            };
            error(code, cause.to_string())
        }
        other => error("persistence_failure", other.to_string()),
    }
}

fn parse_identity(status: &Value) -> Result<ProcessIdentity, ProtocolErrorBody> {
    serde_json::from_value(
        status
            .get("process_identity")
            .cloned()
            .ok_or_else(|| error("host_contract", "session status has no process identity"))?,
    )
    .map_err(|failure| error("host_contract", failure.to_string()))
}

fn session_record(
    session_id: &str,
    status: &Value,
    state: &str,
    now: i64,
) -> Result<SessionRecord, ProtocolErrorBody> {
    Ok(SessionRecord {
        id: session_id.into(),
        owner: "local".into(),
        state: state.into(),
        identity: Some(parse_identity(status)?),
        updated_at_ms: now,
    })
}

fn task_json(task: &TaskRecord) -> Value {
    json!({
        "task_id": task.id,
        "session_id": task.session_id,
        "process_identity": task.identity,
        "state": task.state.as_str(),
        "exit_code": task.exit_code,
        "exit_signal": task.exit_signal,
        "started_at_ms": task.started_at_ms,
        "ended_at_ms": task.ended_at_ms
    })
}

fn lease_json(lease: &LeaseRecord) -> Value {
    json!({
        "session_id": lease.session_id,
        "owner": lease.owner,
        "generation": lease.generation,
        "deadline_ms": lease.deadline_ms,
        "updated_at_ms": lease.updated_at_ms,
        "released_at_ms": lease.released_at_ms,
        "state": if lease.released_at_ms.is_some() { "released" } else { "active" }
    })
}

fn create_session(context: &Context, payload: Value) -> Result<Value, ProtocolErrorBody> {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing session_id"))?
        .to_string();
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = store.find_session(&session_id).map_err(storage_error)? {
        return Err(error(
            "already_exists",
            format!(
                "session '{session_id}' already exists in state '{}'",
                existing.state
            ),
        ));
    }

    let mut status = host_call(context, "SESSION_CREATE", payload)?;
    let persisted = (|| {
        let identity = parse_identity(&status)?;
        let now = now_ms();
        let running = status.get("running").and_then(Value::as_bool) == Some(true);
        let session_state = if running { "running" } else { "exited" };
        let task_state = if running {
            TaskState::Running
        } else {
            TaskState::Exited
        };
        let task_id = TaskId::new();
        store
            .insert_created_session(
                &session_record(&session_id, &status, session_state, now)?,
                &TaskRecord {
                    id: task_id.clone(),
                    session_id: session_id.clone(),
                    identity,
                    state: task_state,
                    exit_code: status
                        .get("exit_code")
                        .and_then(Value::as_i64)
                        .map(|value| value as i32),
                    exit_signal: status
                        .get("exit_signal")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    started_at_ms: now,
                    ended_at_ms: (!running).then_some(now),
                },
            )
            .map_err(storage_error)?;
        status["task_id"] = json!(task_id);
        Ok::<(), ProtocolErrorBody>(())
    })();
    if let Err(failure) = persisted {
        let cleanup = host_call(context, "SESSION_CLOSE", json!({"session_id": session_id}));
        if let Err(cleanup_failure) = cleanup {
            return Err(error(
                &failure.code,
                format!(
                    "{}; rollback also failed: {}: {}",
                    failure.message, cleanup_failure.code, cleanup_failure.message
                ),
            ));
        }
        return Err(failure);
    }
    Ok(status)
}

fn update_task_from_status(
    store: &mut Store,
    task: &TaskRecord,
    status: &Value,
) -> Result<TaskRecord, ProtocolErrorBody> {
    if matches!(
        task.state,
        TaskState::Exited | TaskState::Stopped | TaskState::Lost
    ) {
        return Ok(task.clone());
    }
    let running = status
        .get("running")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = if running {
        TaskState::Running
    } else {
        TaskState::Exited
    };
    let exit_code = status
        .get("exit_code")
        .and_then(Value::as_i64)
        .map(|value| value as i32);
    let exit_signal = status
        .get("exit_signal")
        .and_then(Value::as_str)
        .map(str::to_string);
    store
        .update_task(
            &task.id,
            state,
            exit_code,
            exit_signal.clone(),
            (!running).then(now_ms),
        )
        .map_err(storage_error)?;
    store
        .find_task(&task.id)
        .map_err(storage_error)?
        .ok_or_else(|| error("persistence_failure", "task disappeared after update"))
}

fn task_status(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    let id = payload
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing task_id"))?;
    let id =
        TaskId::from_str(id).map_err(|failure| error("invalid_request", failure.to_string()))?;
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let task = store
        .find_task(&id)
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "task not found"))?;
    let status = host_call(
        context,
        "SESSION_STATUS",
        json!({"session_id": task.session_id}),
    )?;
    let task = update_task_from_status(&mut store, &task, &status)?;
    store
        .upsert_session(&session_record(
            &task.session_id,
            &status,
            if status.get("running").and_then(Value::as_bool) == Some(true) {
                "running"
            } else {
                "exited"
            },
            now_ms(),
        )?)
        .map_err(storage_error)?;
    Ok(json!({
        "task": task_json(&task),
        "terminal": status
    }))
}

fn task_stop(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    let id = payload
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing task_id"))?;
    let id =
        TaskId::from_str(id).map_err(|failure| error("invalid_request", failure.to_string()))?;
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let task = store
        .find_task(&id)
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "task not found"))?;
    let status = host_call(context, "TASK_STOP", json!({"session_id": task.session_id}))?;
    let exit_code = status
        .get("exit_code")
        .and_then(Value::as_i64)
        .map(|value| value as i32);
    let exit_signal = status
        .get("exit_signal")
        .and_then(Value::as_str)
        .map(str::to_string);
    store
        .update_task(
            &task.id,
            TaskState::Stopped,
            exit_code,
            exit_signal,
            Some(now_ms()),
        )
        .map_err(storage_error)?;
    store
        .upsert_session(&session_record(
            &task.session_id,
            &status,
            "exited",
            now_ms(),
        )?)
        .map_err(storage_error)?;
    let task = store
        .find_task(&task.id)
        .map_err(storage_error)?
        .ok_or_else(|| error("persistence_failure", "task disappeared after stop"))?;
    Ok(task_json(&task))
}

fn lease_operation(
    context: &Context,
    operation: &str,
    payload: &Value,
) -> Result<Value, ProtocolErrorBody> {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing session_id"))?;
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = match operation {
        "LEASE_STATUS" => store
            .get_lease(session_id)
            .map_err(storage_error)?
            .map_or_else(
                || json!({"session_id": session_id, "state": "unheld"}),
                |lease| lease_json(&lease),
            ),
        "LEASE_CLAIM" | "LEASE_TAKEOVER" => {
            let owner = required_string(payload, "owner")?;
            let ttl_ms = required_i64(payload, "ttl_ms")?;
            let lease = if operation == "LEASE_CLAIM" {
                store.claim_lease(session_id, owner, ttl_ms, now_ms())
            } else {
                store.takeover_lease(session_id, owner, ttl_ms, now_ms())
            }
            .map_err(storage_error)?;
            lease_json(&lease)
        }
        "LEASE_TOUCH" => {
            let owner = required_string(payload, "owner")?;
            let generation = required_u64(payload, "generation")?;
            let ttl_ms = required_i64(payload, "ttl_ms")?;
            lease_json(
                &store
                    .touch_lease(session_id, owner, generation, ttl_ms, now_ms())
                    .map_err(storage_error)?,
            )
        }
        "LEASE_RELEASE" => {
            let owner = required_string(payload, "owner")?;
            let generation = required_u64(payload, "generation")?;
            lease_json(
                &store
                    .release_lease(session_id, owner, generation, now_ms())
                    .map_err(storage_error)?,
            )
        }
        _ => return Err(error("unsupported_operation", operation)),
    };
    Ok(result)
}

fn required_string<'a>(payload: &'a Value, key: &str) -> Result<&'a str, ProtocolErrorBody> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error("invalid_request", format!("missing {key}")))
}

fn required_i64(payload: &Value, key: &str) -> Result<i64, ProtocolErrorBody> {
    let value = payload
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| error("invalid_request", format!("missing {key}")))?;
    if value <= 0 {
        return Err(error("invalid_request", format!("{key} must be positive")));
    }
    Ok(value)
}

fn required_u64(payload: &Value, key: &str) -> Result<u64, ProtocolErrorBody> {
    payload
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| error("invalid_request", format!("missing {key}")))
}

fn reconcile(context: &Context) -> Result<Reconciliation, Box<dyn std::error::Error>> {
    let now = now_ms();
    let inventory = host_call(context, "HOST_LIST", json!({}))
        .map_err(|failure| format!("{}: {}", failure.code, failure.message))?;
    let sessions = inventory
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or("host inventory has no sessions")?;
    let live_ids: Vec<String> = sessions
        .iter()
        .filter_map(|status| status.get("session_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let mut report = Reconciliation {
        live_sessions: sessions.len(),
        ..Reconciliation::default()
    };
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for status in sessions {
        let Some(session_id) = status.get("session_id").and_then(Value::as_str) else {
            continue;
        };
        let running = status
            .get("running")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let Some(existing) = store.find_session(session_id)? else {
            report.orphaned_sessions += 1;
            continue;
        };
        let state = if running { "running" } else { "exited" };
        if !session_transition_is_valid(&existing.state, state) {
            continue;
        }
        let record = session_record(session_id, status, state, now).map_err(|failure| {
            std::io::Error::other(format!("{}: {}", failure.code, failure.message))
        })?;
        store.upsert_session(&record)?;
        if let Some(task) = store.find_task_for_session(session_id)? {
            let updated = update_task_from_status(&mut store, &task, status)
                .map_err(|failure| format!("{}: {}", failure.code, failure.message))?;
            if updated.state == TaskState::Exited && task.state == TaskState::Running {
                report.exited_tasks += 1;
            }
        }
    }
    report.lost_sessions = store.mark_sessions_missing(&live_ids, now)?;
    for task in store.list_tasks()? {
        if task.state == TaskState::Running && !live_ids.contains(&task.session_id) {
            store.update_task(&task.id, TaskState::Lost, None, None, Some(now))?;
            report.lost_tasks += 1;
        }
    }
    Ok(report)
}

fn session_transition_is_valid(current: &str, next: &str) -> bool {
    if current == next {
        return true;
    }
    let current = match current {
        "creating" => SessionState::Creating,
        "running" => SessionState::Running,
        "exited" => SessionState::Exited,
        "lost" => SessionState::Lost,
        "closing" | "stopped" => SessionState::Closing,
        "closed" => SessionState::Closed,
        _ => return false,
    };
    let next = match next {
        "running" => SessionState::Running,
        "exited" => SessionState::Exited,
        "lost" => SessionState::Lost,
        "closing" => SessionState::Closing,
        "closed" => SessionState::Closed,
        _ => return false,
    };
    current.can_transition_to(next)
}

fn client_operation(
    context: &Context,
    request: &RequestEnvelope,
) -> Result<Value, ProtocolErrorBody> {
    match request.operation.as_str() {
        "CONTROL_STATUS" => {
            let store = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let reconciliation = context
                .reconciliation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            Ok(json!({
                "daemon": "ok",
                "epoch": context.epoch.load(Ordering::SeqCst),
                "reconciliation": reconciliation,
                "sessions": store.list_sessions().map_err(storage_error)?,
                "tasks": store.list_tasks().map_err(storage_error)?
            }))
        }
        "TASK_START" => create_session(context, request.payload.clone()),
        "TASK_STATUS" => task_status(context, &request.payload),
        "TASK_STOP" => task_stop(context, &request.payload),
        "TASK_HISTORY" => {
            let store = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Ok(json!({
                "tasks": store
                    .list_tasks()
                    .map_err(storage_error)?
                    .iter()
                    .map(task_json)
                    .collect::<Vec<_>>()
            }))
        }
        "LEASE_STATUS" | "LEASE_CLAIM" | "LEASE_TOUCH" | "LEASE_RELEASE" | "LEASE_TAKEOVER" => {
            lease_operation(context, &request.operation, &request.payload)
        }
        "SESSION_CREATE" => create_session(context, request.payload.clone()),
        "SESSION_CLOSE" => {
            let session_id = required_string(&request.payload, "session_id")?.to_string();
            let result = host_call(context, "SESSION_CLOSE", request.payload.clone())?;
            let mut store = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(task) = store
                .find_task_for_session(&session_id)
                .map_err(storage_error)?
            {
                if task.state == TaskState::Running {
                    store
                        .update_task(&task.id, TaskState::Stopped, None, None, Some(now_ms()))
                        .map_err(storage_error)?;
                }
            }
            if let Some(mut session) = store.find_session(&session_id).map_err(storage_error)? {
                session.state = "closed".into();
                session.updated_at_ms = now_ms();
                store.upsert_session(&session).map_err(storage_error)?;
            }
            Ok(result)
        }
        "SESSION_STATUS" | "SESSION_ATTACH" | "SESSION_INPUT" | "PING" | "HOST_LIST" => {
            host_call(context, &request.operation, request.payload.clone())
        }
        _ => Err(error("unsupported_operation", request.operation.clone())),
    }
}

fn proxy(mut client: UnixStream, context: &Context) -> Result<(), Box<dyn std::error::Error>> {
    verify_same_uid(&client)?;
    let request: RequestEnvelope = read_frame(&mut client)?;
    validate_version(request.version)
        .map_err(|failure| std::io::Error::other(format!("incompatible_version: {failure}")))?;
    let response = ResponseEnvelope {
        version: PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        result: client_operation(context, &request),
    };
    write_frame(&mut client, &response)?;
    Ok(())
}

fn attach_error(code: &str, message: impl Into<String>) -> attach::AttachError {
    attach::AttachError {
        protocol_version: ATTACH_PROTOCOL_VERSION,
        code: code.into(),
        message: message.into(),
    }
}

fn proxy_bidirectional(mut left: UnixStream, mut right: UnixStream) -> std::io::Result<()> {
    let mut left_reader = left.try_clone()?;
    let mut right_writer = right.try_clone()?;
    let forward = thread::spawn(move || {
        let result = std::io::copy(&mut left_reader, &mut right_writer);
        let _ = right_writer.shutdown(std::net::Shutdown::Write);
        result
    });
    let reverse = std::io::copy(&mut right, &mut left);
    let _ = left.shutdown(std::net::Shutdown::Both);
    let _ = forward.join();
    reverse.map(|_| ())
}

fn serve_attachment(
    mut client: UnixStream,
    context: &Context,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if verify_same_uid(&client).is_err() {
        return Ok(());
    }
    let frame = match attach::read_frame(&mut client) {
        Ok(frame) => frame,
        Err(failure) => {
            let _ = attach::write_json_frame(
                &mut client,
                attach::FrameType::Error,
                &attach_error("unknown_frame_type", failure.to_string()),
            );
            return Ok(());
        }
    };
    if frame.frame_type != attach::FrameType::AttachOpen {
        let _ = attach::write_json_frame(
            &mut client,
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
                &mut client,
                attach::FrameType::Error,
                &attach_error("worker_protocol_error", failure.to_string()),
            );
            return Ok(());
        }
    };
    if open.protocol_version != ATTACH_PROTOCOL_VERSION {
        let _ = attach::write_json_frame(
            &mut client,
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
    if open.mode == attach::AttachMode::ReadOnly {
        let _ = attach::write_json_frame(
            &mut client,
            attach::FrameType::Error,
            &attach_error(
                "unsupported_mode",
                "read_only attach mode is reserved and is not supported in V0",
            ),
        );
        return Ok(());
    }

    let session_state = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .find_session(&open.session_id)?
        .map(|session| session.state);
    if !matches!(session_state.as_deref(), Some("running" | "exited")) {
        let message = session_state.map_or_else(
            || "session has no durable record".to_string(),
            |state| format!("session is in terminal or unavailable state '{state}'"),
        );
        let _ = attach::write_json_frame(
            &mut client,
            attach::FrameType::Error,
            &attach_error("session_lost", message),
        );
        return Ok(());
    }

    let host_attach = attach_socket(&context.host_socket);
    let mut host = match UnixStream::connect(&host_attach) {
        Ok(host) => host,
        Err(failure) => {
            let _ = attach::write_json_frame(
                &mut client,
                attach::FrameType::Error,
                &attach_error("worker_unreachable", format!("{host_attach}: {failure}")),
            );
            return Ok(());
        }
    };
    verify_same_uid(&host)
        .map_err(|failure| format!("host attach peer verification failed: {failure}"))?;
    if let Err(failure) = attach::write_payload_frame(&mut host, frame.frame_type, &frame.payload) {
        let _ = attach::write_json_frame(
            &mut client,
            attach::FrameType::Error,
            &attach_error("worker_protocol_error", failure.to_string()),
        );
        return Ok(());
    }
    proxy_bidirectional(client, host)?;
    Ok(())
}

fn existing_instance(socket: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let request = RequestEnvelope {
        version: PROTOCOL_VERSION,
        request_id: request_id("qwed-singleton"),
        idempotency_key: None,
        host_epoch: Some(1),
        operation: "PING".into(),
        payload: json!({}),
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
    let attach_path = attach_socket(&args.socket);
    let _ = fs::remove_file(&args.socket);
    let _ = fs::remove_file(&attach_path);
    let listener = UnixListener::bind(&args.socket)?;
    fs::set_permissions(&args.socket, fs::Permissions::from_mode(0o600))?;
    let attach_listener = UnixListener::bind(&attach_path)?;
    fs::set_permissions(&attach_path, fs::Permissions::from_mode(0o600))?;
    let store_path = args
        .store
        .unwrap_or_else(|| format!("{}.sqlite", args.socket));
    let store = Store::open(&store_path)?;
    let epoch = register(&args.host_socket)?;
    let context = Context {
        host_socket: args.host_socket,
        epoch: AtomicU64::new(epoch),
        store: Mutex::new(store),
        reconciliation: Mutex::new(json!({})),
    };
    let report = reconcile(&context)?;
    *context
        .reconciliation
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = json!({
        "live_sessions": report.live_sessions,
        "orphaned_sessions": report.orphaned_sessions,
        "lost_sessions": report.lost_sessions,
        "exited_tasks": report.exited_tasks,
        "lost_tasks": report.lost_tasks,
        "checked_at_ms": now_ms()
    });
    let context = Arc::new(context);
    let monitor_context = Arc::clone(&context);
    thread::spawn(move || {
        loop {
            thread::sleep(RECONCILE_INTERVAL);
            if let Ok(report) = reconcile(&monitor_context) {
                *monitor_context
                    .reconciliation
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = json!({
                    "live_sessions": report.live_sessions,
                    "orphaned_sessions": report.orphaned_sessions,
                    "lost_sessions": report.lost_sessions,
                    "exited_tasks": report.exited_tasks,
                    "lost_tasks": report.lost_tasks,
                    "checked_at_ms": now_ms()
                });
            }
        }
    });
    let control_context = Arc::clone(&context);
    thread::spawn(move || {
        for client in listener.incoming() {
            let context = Arc::clone(&control_context);
            thread::spawn(move || {
                if let Err(failure) = proxy(client?, &context) {
                    eprintln!("proxy failed: {failure}");
                }
                Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
            });
        }
    });
    for client in attach_listener.incoming() {
        let context = Arc::clone(&context);
        thread::spawn(move || {
            if let Err(failure) = serve_attachment(client?, &context) {
                eprintln!("attach proxy failed: {failure}");
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
    }
    Ok(())
}
