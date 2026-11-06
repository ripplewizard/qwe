mod capabilities;
mod credentials;
mod worker_client;

use capabilities::{matrix as capability_matrix, merge_resource_policy, validate_resource_policy};
use clap::Parser;
use credentials::CredentialProvider;
use qwe_core::{
    AttachmentAuthority, AttachmentAuthorityState, AttachmentOperation, AuthorityCredentials,
    DEFAULT_RECLAIM_GRACE_MS, LeaseRecord, MAX_LEASE_TTL_MS, PaneId, PaneRecord, ProcessIdentity,
    ReclaimTokenHash, ReconciliationDisposition, ResourcePolicy, SessionRecord, TaskId, TaskRecord,
    TaskState, WorkerHealth,
};
use qwe_protocol::{
    ATTACH_PROTOCOL_VERSION, PROTOCOL_VERSION, ProtocolErrorBody, RequestEnvelope,
    ResponseEnvelope, attach, read_frame, validate_version, verify_same_uid, write_frame,
};
use qwe_storage::{
    HISTORY_MAX_TERMINAL_SESSIONS, HISTORY_RETENTION_MS, IDEMPOTENCY_COMPLETE, LeaseError,
    StorageError, Store,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use worker_client::{WorkerClient, WorkerEvent};
use zeroize::Zeroizing;

const WORKER_CALL_TIMEOUT: Duration = Duration::from_millis(500);
const WORKER_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const WORKER_MAINTENANCE_TIMEOUT: Duration = Duration::from_secs(5);
const SAFETY_SCAN_INTERVAL: Duration = Duration::from_secs(2);
const HISTORY_PRUNE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const WORKER_START_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CONTROL_INPUT_BYTES: usize = 1024 * 1024;

static SIGCHLD_WRITE_FD: AtomicI32 = AtomicI32::new(-1);
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(version, about = "QWE durable terminal supervisor")]
struct Args {
    #[arg(long)]
    socket: String,
    #[arg(long)]
    store: Option<String>,
    #[arg(long)]
    credentials_dir: Option<PathBuf>,
    #[arg(long)]
    keychain_service_prefix: Option<String>,
    #[arg(long)]
    keychain_path: Option<PathBuf>,
}

struct Context {
    runtime_dir: PathBuf,
    linux_scope_available: bool,
    store: Mutex<Store>,
    workers: Mutex<HashMap<String, Arc<WorkerHandle>>>,
    reconciliation_incidents: Mutex<ReconciliationIncidents>,
    last_scan_ms: AtomicI64,
    worker_events: Sender<WorkerEvent>,
    creation_lock: Mutex<()>,
    idempotency_lock: Mutex<()>,
    attachment_lock: Mutex<()>,
    credentials: CredentialProvider,
    _runtime_lock: File,
}

struct WorkerHandle {
    socket: PathBuf,
    attach_socket: PathBuf,
    worker_pid: u32,
    worker_identity: ProcessIdentity,
    client: Option<Arc<WorkerClient>>,
    health_client: Option<Arc<WorkerClient>>,
    health: Mutex<WorkerHealth>,
}

#[derive(Debug)]
enum SupervisorEvent {
    Worker(WorkerEvent),
    ChildExited { pid: u32 },
}

#[derive(Default)]
struct Reconciliation {
    orphaned_sessions: usize,
    conflict_sessions: usize,
    incidents: Vec<RuntimeIncident>,
}

#[derive(Default, Clone)]
struct ReconciliationIncidents {
    orphaned_sessions: usize,
    conflict_sessions: usize,
    incidents: Vec<RuntimeIncident>,
}

#[derive(Debug, Clone)]
struct RuntimeIncident {
    session_id: Option<String>,
    disposition: ReconciliationDisposition,
    health: WorkerHealth,
    socket: PathBuf,
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

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_decode(value: &str) -> Option<String> {
    if value.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(value.get(index..index + 2)?, 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

fn worker_socket(runtime_dir: &Path, session_id: &str) -> PathBuf {
    runtime_dir.join(format!("{}.sock", hex_encode(session_id.as_bytes())))
}

fn worker_attach_socket(runtime_dir: &Path, session_id: &str) -> PathBuf {
    runtime_dir.join(format!("{}.a", hex_encode(session_id.as_bytes())))
}

fn public_attach_socket(socket: &str) -> String {
    format!("{socket}.attach")
}

fn acquire_runtime_lock(runtime_dir: &Path) -> Result<File, Box<dyn std::error::Error>> {
    fs::create_dir_all(runtime_dir)?;
    fs::set_permissions(runtime_dir, fs::Permissions::from_mode(0o700))?;
    let lock_path = runtime_dir.join("qwed.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(format!(
            "qwed runtime lock is already held at {}: {}",
            runtime_dir.display(),
            std::io::Error::last_os_error()
        )
        .into());
    }
    Ok(file)
}

fn mark_subreaper() {
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }
}

fn worker_executable() -> Result<PathBuf, std::io::Error> {
    Ok(std::env::current_exe()?.with_file_name("qwe-worker"))
}

fn parse_worker_identity(status: &Value) -> Result<(u32, ProcessIdentity), ProtocolErrorBody> {
    let worker_pid = status
        .get("worker_pid")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| error("worker_contract", "worker status has no worker_pid"))?;
    let worker_identity: ProcessIdentity = serde_json::from_value(
        status
            .get("worker_identity")
            .cloned()
            .ok_or_else(|| error("worker_contract", "worker status has no worker identity"))?,
    )
    .map_err(|failure| error("worker_contract", failure.to_string()))?;
    if worker_identity.pid != worker_pid {
        return Err(error(
            "worker_contract",
            "worker PID does not match its process identity",
        ));
    }
    Ok((worker_pid, worker_identity))
}

fn validate_worker_status(
    session_id: &str,
    expected_socket: &Path,
    status: &Value,
) -> Result<(u32, ProcessIdentity), ProtocolErrorBody> {
    if status.get("session_id").and_then(Value::as_str) != Some(session_id) {
        return Err(error(
            "worker_contract",
            "worker reported the wrong SessionId",
        ));
    }
    let (worker_pid, identity) = parse_worker_identity(status)?;
    let actual_socket = worker_socket(
        expected_socket
            .parent()
            .expect("worker socket has a runtime parent"),
        session_id,
    );
    if actual_socket != expected_socket {
        return Err(error(
            "worker_contract",
            "worker socket path does not match the Session",
        ));
    }
    if !qwe_runtime::process_matches(&identity) {
        return Err(error(
            "worker_contract",
            "worker PID or start token is stale",
        ));
    }
    Ok((worker_pid, identity))
}

fn worker_handle(
    session_id: &str,
    socket: PathBuf,
    status: &Value,
    client: Option<Arc<WorkerClient>>,
    health_client: Option<Arc<WorkerClient>>,
    health: WorkerHealth,
) -> Result<WorkerHandle, ProtocolErrorBody> {
    let (worker_pid, worker_identity) = validate_worker_status(session_id, &socket, status)?;
    Ok(WorkerHandle {
        attach_socket: worker_attach_socket(
            socket.parent().expect("worker socket has a runtime parent"),
            session_id,
        ),
        socket,
        worker_pid,
        worker_identity,
        client,
        health_client,
        health: Mutex::new(health),
    })
}

fn set_worker_health(handle: &WorkerHandle, health: WorkerHealth) {
    *handle
        .health
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = health;
}

fn worker_health(handle: &WorkerHandle) -> WorkerHealth {
    *handle
        .health
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn worker_json(session_id: &str, handle: &WorkerHandle) -> Value {
    json!({
        "session_id": session_id,
        "pid": handle.worker_pid,
        "identity": handle.worker_identity,
        "health": worker_health(handle).as_str()
    })
}

fn unreachable_worker_handle(
    context: &Context,
    session: &SessionRecord,
) -> Option<Arc<WorkerHandle>> {
    let worker_identity = session.worker_identity.clone()?;
    let socket = worker_socket(&context.runtime_dir, &session.id);
    Some(Arc::new(WorkerHandle {
        attach_socket: worker_attach_socket(&context.runtime_dir, &session.id),
        worker_pid: worker_identity.pid,
        worker_identity,
        socket,
        client: None,
        health_client: None,
        health: Mutex::new(WorkerHealth::Unreachable),
    }))
}

fn remember_unreachable(context: &Context, session: &SessionRecord) {
    if let Some(handle) = unreachable_worker_handle(context, session) {
        context
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(session.id.clone(), handle);
    }
}

fn call_worker(
    context: &Context,
    session_id: &str,
    operation: &str,
    payload: Value,
    timeout: Duration,
) -> Result<Value, ProtocolErrorBody> {
    let handle = context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(session_id)
        .cloned()
        .ok_or_else(|| error("terminal_lost", "session has no live worker"))?;
    if worker_health(&handle) == WorkerHealth::Unreachable {
        return Err(error(
            "worker_unreachable",
            "session worker is alive but not responding",
        ));
    }
    let client = handle.client.as_ref().ok_or_else(|| {
        error(
            "worker_unreachable",
            "session worker has no active control channel",
        )
    })?;
    match client.call(operation, payload, timeout) {
        Ok(status) => {
            set_worker_health(&handle, WorkerHealth::Healthy);
            Ok(status)
        }
        Err(failure) => {
            if failure.code == "worker_unreachable" {
                set_worker_health(&handle, WorkerHealth::Unreachable);
            }
            Err(failure)
        }
    }
}

fn connect_existing_worker(
    context: &Context,
    session_id: &str,
    socket: PathBuf,
) -> Result<(Arc<WorkerClient>, Value), String> {
    let client = WorkerClient::connect(&socket, session_id, context.worker_events.clone())
        .map_err(|failure| failure.to_string())?;
    let status = client
        .call("WORKER_HELLO", json!({}), WORKER_CALL_TIMEOUT)
        .map_err(|failure| format!("{}: {}", failure.code, failure.message))?;
    Ok((client, status))
}

fn connect_health_client(
    context: &Context,
    session_id: &str,
    socket: &Path,
) -> Result<Arc<WorkerClient>, String> {
    let client = WorkerClient::connect(socket, session_id, context.worker_events.clone())
        .map_err(|failure| failure.to_string())?;
    client
        .call("PING", json!({}), WORKER_CALL_TIMEOUT)
        .map_err(|failure| format!("{}: {}", failure.code, failure.message))?;
    Ok(client)
}

fn spawn_worker(
    context: &Context,
    session_id: &str,
    program: &str,
    arguments: &[String],
    resource_policy: &ResourcePolicy,
    credential: Option<(&str, &Zeroizing<Vec<u8>>)>,
) -> Result<(WorkerHandle, Value), ProtocolErrorBody> {
    let socket = worker_socket(&context.runtime_dir, session_id);
    let attach_socket = worker_attach_socket(&context.runtime_dir, session_id);
    let _ = fs::remove_file(&socket);
    let _ = fs::remove_file(&attach_socket);
    let executable =
        worker_executable().map_err(|failure| error("internal", failure.to_string()))?;
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
    if context.linux_scope_available {
        command.arg("--worker-systemd-scope");
    }
    if let Some(seconds) = resource_policy.cpu_seconds {
        command.arg("--worker-cpu-seconds").arg(seconds.to_string());
    }
    if let Some(percent) = resource_policy.cpu_quota_percent {
        command
            .arg("--worker-cpu-quota-percent")
            .arg(percent.to_string());
    }
    if let Some(bytes) = resource_policy.memory_high_bytes {
        command
            .arg("--worker-memory-high-bytes")
            .arg(bytes.to_string());
    }
    if let Some(bytes) = resource_policy.memory_max_bytes {
        command.arg("--worker-memory-bytes").arg(bytes.to_string());
    }
    if let Some(bytes) = resource_policy.memory_swap_max_bytes {
        command
            .arg("--worker-memory-swap-max-bytes")
            .arg(bytes.to_string());
    }
    if let Some(tasks) = resource_policy.tasks_max {
        command.arg("--worker-tasks-max").arg(tasks.to_string());
    }
    let credential_fd = credential
        .map(|(_, secret)| credential_pipe(secret))
        .transpose()
        .map_err(|failure| error("credential_injection_failed", failure.to_string()))?;
    if let (Some((_name, _secret)), Some(fd)) = (credential, credential_fd.as_ref()) {
        command
            .arg("--worker-credential-fd")
            .arg(fd.as_raw_fd().to_string());
    }
    let child = command
        .spawn()
        .map_err(|failure| error("worker_start_failed", failure.to_string()))?;
    drop(credential_fd);
    let mut child = child;
    let deadline = Instant::now() + WORKER_START_TIMEOUT;
    let mut last_failure = "worker did not create a control socket".to_string();
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(error(
                "worker_start_failed",
                format!("worker exited during startup with {status}"),
            ));
        }
        match connect_existing_worker(context, session_id, socket.clone()) {
            Ok((client, status)) => {
                let health_client = match connect_health_client(context, session_id, &socket) {
                    Ok(client) => client,
                    Err(failure) => {
                        last_failure = failure;
                        thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                };
                return worker_handle(
                    session_id,
                    socket,
                    &status,
                    Some(client),
                    Some(health_client),
                    WorkerHealth::Healthy,
                )
                .map(|handle| (handle, status));
            }
            Err(failure) => last_failure = failure.to_string(),
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&socket);
    let _ = fs::remove_file(&attach_socket);
    Err(error("worker_start_failed", last_failure))
}

fn credential_pipe(secret: &[u8]) -> std::io::Result<OwnedFd> {
    let mut descriptors = [-1_i32; 2];
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut writer = unsafe { File::from_raw_fd(descriptors[1]) };
    if let Err(failure) = writer.write_all(secret).and_then(|()| writer.flush()) {
        unsafe {
            libc::close(descriptors[0]);
        }
        return Err(failure);
    }
    drop(writer);
    Ok(unsafe { OwnedFd::from_raw_fd(descriptors[0]) })
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

fn parse_process_identity(status: &Value) -> Result<ProcessIdentity, ProtocolErrorBody> {
    serde_json::from_value(
        status
            .get("process_identity")
            .cloned()
            .ok_or_else(|| error("worker_contract", "worker status has no process identity"))?,
    )
    .map_err(|failure| error("worker_contract", failure.to_string()))
}

fn session_record(
    session_id: &str,
    status: &Value,
    worker_identity: &ProcessIdentity,
    state: &str,
    now: i64,
) -> Result<SessionRecord, ProtocolErrorBody> {
    Ok(SessionRecord {
        id: session_id.into(),
        owner: "local".into(),
        state: state.into(),
        identity: Some(parse_process_identity(status)?),
        worker_identity: Some(worker_identity.clone()),
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
    let _creation = context
        .creation_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error("invalid_request", "missing session_id"))?
        .to_string();
    {
        let store = context
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
    }
    let program = payload
        .get("program")
        .and_then(Value::as_str)
        .unwrap_or("sh");
    let arguments = payload
        .get("args")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let resource_policy = validate_resource_policy(&payload, context.linux_scope_available)?;
    let credential_name = match payload.get("credential") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if value.is_empty() => None,
        Some(Value::String(value)) => Some(value.as_str()),
        Some(_) => {
            return Err(error("invalid_request", "credential must be a string"));
        }
    };
    let credential = credential_name
        .map(|name| {
            context
                .credentials
                .read(name)
                .map(|secret| (name.to_string(), secret))
                .map_err(|failure| error("credential_unavailable", failure.to_string()))
        })
        .transpose()?;
    let (handle, mut status) = spawn_worker(
        context,
        &session_id,
        program,
        &arguments,
        &resource_policy,
        credential
            .as_ref()
            .map(|(name, secret)| (name.as_str(), secret)),
    )?;
    let now = now_ms();
    let running = status.get("running").and_then(Value::as_bool) == Some(true);
    let session_state = if running { "running" } else { "exited" };
    let task_state = if running {
        TaskState::Running
    } else {
        TaskState::Exited
    };
    let task_id = TaskId::new();
    let pane_id = PaneId::new();
    let persisted = (|| {
        let identity = parse_process_identity(&status)?;
        let session = session_record(
            &session_id,
            &status,
            &handle.worker_identity,
            session_state,
            now,
        )?;
        let pane = PaneRecord {
            id: pane_id.clone(),
            session_id: session_id.clone(),
            resource_policy: resource_policy.clone(),
            updated_at_ms: now,
        };
        let task = TaskRecord {
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
        };
        context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert_created_session(&session, &pane, &task)
            .map_err(storage_error)
    })();
    if let Err(failure) = persisted {
        if let Some(client) = &handle.client {
            let _ = client.call(
                "SESSION_CLOSE",
                json!({"session_id": session_id}),
                WORKER_MAINTENANCE_TIMEOUT,
            );
        }
        unsafe {
            libc::kill(handle.worker_pid as libc::pid_t, libc::SIGTERM);
        }
        let _ = fs::remove_file(&handle.socket);
        let _ = fs::remove_file(&handle.attach_socket);
        return Err(failure);
    }
    context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(session_id, Arc::new(handle));
    status["task_id"] = json!(task_id);
    status["pane_id"] = json!(pane_id);
    status["resource_policy"] = json!(resource_policy);
    if let Some(response) = status.as_object_mut() {
        response.remove("total_written");
        response.remove("truncated_from");
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
            exit_signal,
            (!running).then(now_ms),
        )
        .map_err(storage_error)?;
    store
        .find_task(&task.id)
        .map_err(storage_error)?
        .ok_or_else(|| error("persistence_failure", "task disappeared after update"))
}

fn apply_status(
    context: &Context,
    session_id: &str,
    status: &Value,
) -> Result<(), ProtocolErrorBody> {
    let handle = context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(session_id)
        .cloned();
    let worker_identity = handle
        .as_ref()
        .map(|handle| handle.worker_identity.clone())
        .or_else(|| {
            context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .find_session(session_id)
                .ok()
                .flatten()
                .and_then(|record| record.worker_identity)
        })
        .ok_or_else(|| error("worker_contract", "missing worker identity"))?;
    let running = status.get("running").and_then(Value::as_bool) == Some(true);
    let state = if running { "running" } else { "exited" };
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    store
        .upsert_session(&session_record(
            session_id,
            status,
            &worker_identity,
            state,
            now_ms(),
        )?)
        .map_err(storage_error)?;
    if let Some(task) = store
        .find_task_for_session(session_id)
        .map_err(storage_error)?
    {
        update_task_from_status(&mut store, &task, status)?;
    }
    Ok(())
}

fn task_status(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    let id = payload
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing task_id"))?;
    let id =
        TaskId::from_str(id).map_err(|failure| error("invalid_request", failure.to_string()))?;
    let session_id = {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        store
            .find_task(&id)
            .map_err(storage_error)?
            .ok_or_else(|| error("not_found", "task not found"))?
            .session_id
    };
    let status = match call_worker(
        context,
        &session_id,
        "SESSION_STATUS",
        json!({"session_id": session_id}),
        WORKER_REQUEST_TIMEOUT,
    ) {
        Ok(status) => status,
        Err(failure) if failure.code == "terminal_lost" => {
            let store = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let closed = store
                .find_session(&session_id)
                .map_err(storage_error)?
                .is_some_and(|session| session.state == "closed");
            if !closed {
                return Err(failure);
            }
            let task = store
                .find_task(&id)
                .map_err(storage_error)?
                .ok_or_else(|| error("not_found", "task not found"))?;
            return Ok(json!({
                "task": task_json(&task),
                "terminal": {
                    "session_id": task.session_id,
                    "running": false,
                    "state": "closed",
                    "exit_code": task.exit_code,
                    "exit_signal": task.exit_signal
                }
            }));
        }
        Err(failure) => return Err(failure),
    };
    apply_status(context, &session_id, &status)?;
    let task = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .find_task(&id)
        .map_err(storage_error)?
        .ok_or_else(|| error("persistence_failure", "task disappeared after update"))?;
    Ok(json!({"task": task_json(&task), "terminal": status}))
}

fn update_session_resources(
    context: &Context,
    payload: &Value,
) -> Result<Value, ProtocolErrorBody> {
    let session_id = required_string(payload, "session_id")?.to_string();
    let update = payload
        .get("resource_policy")
        .ok_or_else(|| error("invalid_request", "missing resource_policy"))?;
    let _authority_guard = context
        .attachment_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (previous, next) = {
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        authorize_session_control(
            &mut store,
            &session_id,
            payload,
            AttachmentOperation::ResourcePolicy,
            now_ms(),
        )?;
        let pane = store
            .find_pane_for_session(&session_id)
            .map_err(storage_error)?
            .ok_or_else(|| error("not_found", "Pane not found"))?;
        let next =
            merge_resource_policy(&pane.resource_policy, update, context.linux_scope_available)?;
        (pane.resource_policy, next)
    };
    let terminal = call_worker(
        context,
        &session_id,
        "SESSION_RESOURCE_UPDATE",
        json!({"session_id": session_id, "resource_policy": next}),
        WORKER_MAINTENANCE_TIMEOUT,
    )?;
    let persisted = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .update_pane_resource_policy(&session_id, &next, now_ms())
        .map_err(storage_error);
    if let Err(failure) = persisted {
        let _ = call_worker(
            context,
            &session_id,
            "SESSION_RESOURCE_UPDATE",
            json!({"session_id": session_id, "resource_policy": previous}),
            WORKER_MAINTENANCE_TIMEOUT,
        );
        return Err(failure);
    }
    Ok(json!({
        "session_id": session_id,
        "resource_policy": next,
        "terminal": terminal
    }))
}

fn persisted_session_status(
    context: &Context,
    session_id: &str,
) -> Result<Value, ProtocolErrorBody> {
    let store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let session = store
        .find_session(session_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "session not found"))?;
    let task = store
        .find_task_for_session(session_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("persistence_failure", "Session Task is missing"))?;
    let pane = store
        .find_pane_for_session(session_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("persistence_failure", "Session Pane is missing"))?;
    Ok(json!({
        "session_id": session.id,
        "pid": session.identity.as_ref().map(|identity| identity.pid),
        "pgid": session.identity.as_ref().map(|identity| identity.pgid),
        "start_token": session.identity.as_ref().map(|identity| identity.start_token.as_str()),
        "process_identity": session.identity,
        "worker_pid": session.worker_identity.as_ref().map(|identity| identity.pid),
        "worker_identity": session.worker_identity,
        "running": false,
        "state": session.state,
        "exit_code": task.exit_code,
        "exit_signal": task.exit_signal,
        "task_id": task.id,
        "pane_id": pane.id,
        "resource_policy": pane.resource_policy
    }))
}

fn authorize_session_control(
    store: &mut Store,
    session_id: &str,
    payload: &Value,
    operation: AttachmentOperation,
    now_ms: i64,
) -> Result<(), ProtocolErrorBody> {
    let mut authority = store
        .get_attachment_authority(session_id)
        .map_err(storage_error)?
        .unwrap_or_else(AttachmentAuthority::unattached);
    if authority.state != authority.state_at(now_ms) {
        authority.release();
        store
            .write_attachment_authority(session_id, &authority, now_ms)
            .map_err(storage_error)?;
    }
    authority
        .authorize(
            operation,
            AuthorityCredentials {
                owner: payload.get("owner").and_then(Value::as_str),
                lease_generation: required_u64(payload, "generation").ok(),
                ..AuthorityCredentials::default()
            },
            now_ms,
        )
        .map_err(|failure| error(failure.code(), failure.to_string()))
}

fn task_stop(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    let id = payload
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing task_id"))?;
    let id =
        TaskId::from_str(id).map_err(|failure| error("invalid_request", failure.to_string()))?;
    let _authority_guard = context
        .attachment_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let task = {
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let task = store
            .find_task(&id)
            .map_err(storage_error)?
            .ok_or_else(|| error("not_found", "task not found"))?;
        authorize_session_control(
            &mut store,
            &task.session_id,
            payload,
            AttachmentOperation::CloseTaskStop,
            now_ms(),
        )?;
        task
    };
    let status = call_worker(
        context,
        &task.session_id,
        "TASK_STOP",
        json!({"session_id": task.session_id}),
        WORKER_MAINTENANCE_TIMEOUT,
    )?;
    let exit_code = status
        .get("exit_code")
        .and_then(Value::as_i64)
        .map(|value| value as i32);
    let exit_signal = status
        .get("exit_signal")
        .and_then(Value::as_str)
        .map(str::to_string);
    let worker_identity = context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&task.session_id)
        .map(|handle| handle.worker_identity.clone())
        .ok_or_else(|| error("terminal_lost", "session worker disappeared"))?;
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            &worker_identity,
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

fn task_kill_batch(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    let requested = payload
        .get("task_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| error("invalid_request", "missing task_ids"))?;
    if requested.is_empty() || requested.len() > 100 {
        return Err(error(
            "invalid_request",
            "task_ids must contain between 1 and 100 TaskIds",
        ));
    }
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for value in requested {
        let raw = value
            .as_str()
            .ok_or_else(|| error("invalid_request", "task_ids entries must be strings"))?;
        let id = TaskId::from_str(raw)
            .map_err(|failure| error("invalid_request", failure.to_string()))?;
        if seen.insert(id.to_string()) {
            ids.push(id);
        }
    }
    let mut killed = Vec::new();
    let mut failures = Vec::new();
    for id in ids {
        let result = (|| {
            let _authority_guard = context
                .attachment_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let task = {
                let mut store = context
                    .store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let task = store
                    .find_task(&id)
                    .map_err(storage_error)?
                    .ok_or_else(|| error("not_found", "task not found"))?;
                authorize_session_control(
                    &mut store,
                    &task.session_id,
                    payload,
                    AttachmentOperation::CloseTaskStop,
                    now_ms(),
                )?;
                task
            };
            let status = match call_worker(
                context,
                &task.session_id,
                "TASK_KILL",
                json!({"session_id": task.session_id}),
                WORKER_MAINTENANCE_TIMEOUT,
            ) {
                Ok(status) => status,
                Err(_) if qwe_runtime::process_matches(&task.identity) => {
                    unsafe {
                        libc::kill(-(task.identity.pgid as libc::pid_t), libc::SIGKILL);
                    }
                    json!({
                        "session_id": task.session_id,
                        "process_identity": task.identity,
                        "running": false,
                        "exit_code": Value::Null,
                        "exit_signal": "SIGKILL"
                    })
                }
                Err(failure) => return Err(failure),
            };
            let worker_identity = context
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&task.session_id)
                .map(|handle| handle.worker_identity.clone())
                .ok_or_else(|| error("terminal_lost", "session worker disappeared"))?;
            let exit_code = status
                .get("exit_code")
                .and_then(Value::as_i64)
                .map(|value| value as i32);
            let exit_signal = status
                .get("exit_signal")
                .and_then(Value::as_str)
                .map(str::to_string);
            let mut store = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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
                    &worker_identity,
                    "exited",
                    now_ms(),
                )?)
                .map_err(storage_error)?;
            store
                .find_task(&task.id)
                .map_err(storage_error)?
                .map(|task| task_json(&task))
                .ok_or_else(|| error("persistence_failure", "task disappeared after kill"))
        })();
        match result {
            Ok(task) => killed.push(task),
            Err(failure) => failures.push(json!({
                "task_id": id,
                "code": failure.code,
                "message": failure.message
            })),
        }
    }
    Ok(json!({"tasks": killed, "failures": failures}))
}

fn teardown_session(
    context: &Context,
    payload: &Value,
    worker_operation: &str,
    forced: bool,
) -> Result<Value, ProtocolErrorBody> {
    let session_id = required_string(payload, "session_id")?.to_string();
    let _authority_guard = context
        .attachment_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    {
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        authorize_session_control(
            &mut store,
            &session_id,
            payload,
            AttachmentOperation::CloseTaskStop,
            now_ms(),
        )?;
    }
    let result = match call_worker(
        context,
        &session_id,
        worker_operation,
        payload.clone(),
        WORKER_MAINTENANCE_TIMEOUT,
    ) {
        Ok(result) => result,
        Err(failure) if forced => {
            if let Some(task) = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .find_task_for_session(&session_id)
                .map_err(storage_error)?
            {
                if qwe_runtime::process_matches(&task.identity) {
                    unsafe {
                        libc::kill(-(task.identity.pgid as libc::pid_t), libc::SIGKILL);
                    }
                }
            }
            json!({
                "session_id": session_id,
                "state": "closed",
                "forced": true,
                "worker_error": failure
            })
        }
        Err(failure) => return Err(failure),
    };
    let handle = context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&session_id);
    if let Some(handle) = &handle {
        unsafe {
            libc::kill(
                handle.worker_pid as libc::pid_t,
                if forced { libc::SIGKILL } else { libc::SIGTERM },
            );
        }
        let _ = fs::remove_file(&handle.socket);
        let _ = fs::remove_file(&handle.attach_socket);
    }
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
    let mut authority = store
        .get_attachment_authority(&session_id)
        .map_err(storage_error)?
        .unwrap_or_else(AttachmentAuthority::unattached);
    if let Some(lease) = store.get_lease(&session_id).map_err(storage_error)? {
        if lease.released_at_ms.is_none() {
            let released = store
                .release_lease(&session_id, &lease.owner, lease.generation, now_ms())
                .map_err(storage_error)?;
            authority.generation = released.generation;
        }
    }
    authority.release();
    store
        .write_attachment_authority(&session_id, &authority, now_ms())
        .map_err(storage_error)?;
    Ok(result)
}

fn close_session(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    teardown_session(context, payload, "SESSION_CLOSE", false)
}

fn kill_session(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    teardown_session(context, payload, "SESSION_KILL", true)
}

fn stop_all_sessions(context: &Context) -> Result<(Vec<String>, Vec<Value>), ProtocolErrorBody> {
    let sessions = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .list_sessions()
        .map_err(storage_error)?;
    let mut closed = Vec::new();
    let mut failures = Vec::new();
    for session in sessions {
        if session.state == "closed" {
            continue;
        }
        let handle = context
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&session.id);
        let task = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .find_task_for_session(&session.id)
            .map_err(storage_error)?;
        let worker_result = handle.as_ref().and_then(|handle| {
            handle.client.as_ref().and_then(|client| {
                client
                    .call(
                        "SESSION_KILL",
                        json!({"session_id": session.id}),
                        WORKER_MAINTENANCE_TIMEOUT,
                    )
                    .ok()
            })
        });
        if worker_result.is_none() {
            if let Some(task) = &task {
                if qwe_runtime::process_matches(&task.identity) {
                    unsafe {
                        libc::kill(-(task.identity.pgid as libc::pid_t), libc::SIGKILL);
                    }
                }
            }
        }
        if let Some(handle) = &handle {
            unsafe {
                libc::kill(handle.worker_pid as libc::pid_t, libc::SIGKILL);
            }
            let _ = fs::remove_file(&handle.socket);
            let _ = fs::remove_file(&handle.attach_socket);
        }
        let now = now_ms();
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let persistence = (|| {
            if let Some(task) = &task {
                let exit_code = worker_result
                    .as_ref()
                    .and_then(|status| status.get("exit_code"))
                    .and_then(Value::as_i64)
                    .map(|value| value as i32);
                let exit_signal = worker_result
                    .as_ref()
                    .and_then(|status| status.get("exit_signal"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| worker_result.is_none().then(|| "SIGKILL".to_string()));
                store
                    .update_task(
                        &task.id,
                        TaskState::Stopped,
                        exit_code,
                        exit_signal,
                        Some(now),
                    )
                    .map_err(storage_error)?;
            }
            if let Some(mut session) = store.find_session(&session.id).map_err(storage_error)? {
                session.state = "closed".into();
                session.updated_at_ms = now;
                store.upsert_session(&session).map_err(storage_error)?;
            }
            if let Some(lease) = store.get_lease(&session.id).map_err(storage_error)? {
                if lease.released_at_ms.is_none() {
                    store
                        .release_lease(&session.id, &lease.owner, lease.generation, now)
                        .map_err(storage_error)?;
                }
            }
            let mut authority = store
                .get_attachment_authority(&session.id)
                .map_err(storage_error)?
                .unwrap_or_else(AttachmentAuthority::unattached);
            authority.release();
            store
                .write_attachment_authority(&session.id, &authority, now)
                .map_err(storage_error)?;
            Ok::<(), ProtocolErrorBody>(())
        })();
        match persistence {
            Ok(()) => closed.push(session.id),
            Err(failure) => failures.push(json!({
                "session_id": session.id,
                "code": failure.code,
                "message": failure.message
            })),
        }
    }
    Ok((closed, failures))
}

fn kill_server(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    if payload.get("confirm").and_then(Value::as_bool) != Some(true) {
        return Err(error(
            "confirmation_required",
            "kill-server requires confirm=true",
        ));
    }
    let (closed, failures) = stop_all_sessions(context)?;
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(300));
        std::process::exit(0);
    });
    Ok(json!({
        "daemon": "stopping",
        "closed_sessions": closed,
        "failures": failures
    }))
}

fn graceful_shutdown(context: &Context, socket: &str) {
    match stop_all_sessions(context) {
        Ok((closed, failures)) => eprintln!(
            "qwed stopping: closed {} Sessions, {} teardown failures",
            closed.len(),
            failures.len()
        ),
        Err(failure) => eprintln!(
            "qwed stopping after teardown error: {}: {}",
            failure.code, failure.message
        ),
    }
    let _ = fs::remove_file(socket);
    let _ = fs::remove_file(public_attach_socket(socket));
}

fn lease_operation(
    context: &Context,
    operation: &str,
    payload: &Value,
) -> Result<Value, ProtocolErrorBody> {
    let session_id = required_string(payload, "session_id")?;
    let _authority_guard = context
        .attachment_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = now_ms();
    let mut authority = store
        .get_attachment_authority(session_id)
        .map_err(storage_error)?
        .unwrap_or_else(AttachmentAuthority::unattached);
    if authority.state != authority.state_at(now) {
        authority.release();
        store
            .write_attachment_authority(session_id, &authority, now)
            .map_err(storage_error)?;
    }
    let authority_operation = match operation {
        "LEASE_CLAIM" | "LEASE_TAKEOVER" => Some(AttachmentOperation::LeaseClaimTakeover),
        "LEASE_TOUCH" | "LEASE_RELEASE" => Some(AttachmentOperation::LeaseTouchRelease),
        _ => None,
    };
    if let Some(authority_operation) = authority_operation {
        authority
            .authorize(
                authority_operation,
                AuthorityCredentials {
                    owner: payload.get("owner").and_then(Value::as_str),
                    lease_generation: required_u64(payload, "generation").ok(),
                    ..AuthorityCredentials::default()
                },
                now,
            )
            .map_err(|failure| error(failure.code(), failure.to_string()))?;
    }
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
                store.claim_lease(session_id, owner, ttl_ms, now)
            } else {
                store.takeover_lease(session_id, owner, ttl_ms, now)
            }
            .map_err(storage_error)?;
            store
                .persist_attachment_hold(
                    session_id,
                    &lease.owner,
                    lease.generation,
                    lease.deadline_ms,
                    now,
                )
                .map_err(storage_error)?;
            lease_json(&lease)
        }
        "LEASE_TOUCH" => {
            let owner = required_string(payload, "owner")?;
            let generation = required_u64(payload, "generation")?;
            let ttl_ms = required_i64(payload, "ttl_ms")?;
            let lease = store
                .touch_lease(session_id, owner, generation, ttl_ms, now)
                .map_err(storage_error)?;
            store
                .persist_attachment_hold(
                    session_id,
                    &lease.owner,
                    lease.generation,
                    lease.deadline_ms,
                    now,
                )
                .map_err(storage_error)?;
            lease_json(&lease)
        }
        "LEASE_RELEASE" => {
            let owner = required_string(payload, "owner")?;
            let generation = required_u64(payload, "generation")?;
            let lease = store
                .release_lease(session_id, owner, generation, now)
                .map_err(storage_error)?;
            authority.generation = lease.generation;
            authority.release();
            store
                .write_attachment_authority(session_id, &authority, now)
                .map_err(storage_error)?;
            lease_json(&lease)
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
    if value > MAX_LEASE_TTL_MS {
        return Err(error(
            "invalid_request",
            format!("{key} must not exceed {MAX_LEASE_TTL_MS}"),
        ));
    }
    Ok(value)
}

fn required_u64(payload: &Value, key: &str) -> Result<u64, ProtocolErrorBody> {
    payload
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| error("invalid_request", format!("missing {key}")))
}

fn mark_worker_lost(context: &Context, session_id: &str) -> Result<(), StorageError> {
    let now = now_ms();
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(mut session) = store.find_session(session_id)? {
        if !matches!(session.state.as_str(), "lost" | "closed") {
            session.state = "lost".into();
            session.updated_at_ms = now;
            store.upsert_session(&session)?;
        }
    }
    if let Some(task) = store.find_task_for_session(session_id)? {
        if matches!(task.state, TaskState::Running) {
            store.update_task(&task.id, TaskState::Lost, None, None, Some(now))?;
        }
    }
    Ok(())
}

fn remove_worker_files(context: &Context, session_id: &str) {
    let _ = fs::remove_file(worker_socket(&context.runtime_dir, session_id));
    let _ = fs::remove_file(worker_attach_socket(&context.runtime_dir, session_id));
}

fn quarantine_worker(context: &Context, session_id: &str) {
    for path in [
        worker_socket(&context.runtime_dir, session_id),
        worker_attach_socket(&context.runtime_dir, session_id),
    ] {
        quarantine_runtime_path(&path);
    }
}

fn quarantine_runtime_path(path: &Path) {
    if !path.exists() {
        return;
    }
    let suffix = format!(
        "conflict-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("runtime-entry");
    let _ = fs::rename(path, path.with_file_name(format!("{file_name}.{suffix}")));
}

fn quarantine_runtime_family(path: &Path) {
    quarantine_runtime_path(path);
    if path.extension().and_then(|value| value.to_str()) == Some("sock") {
        quarantine_runtime_path(&path.with_extension("a"));
    }
}

fn probe_runtime_worker(context: &Context, socket: &Path) -> Option<Value> {
    let client = WorkerClient::connect(
        socket,
        "runtime-reconciliation",
        context.worker_events.clone(),
    )
    .ok()?;
    client
        .call("WORKER_HELLO", json!({}), WORKER_CALL_TIMEOUT)
        .ok()
}

fn runtime_incident_json(incident: &RuntimeIncident) -> Value {
    json!({
        "session_id": incident.session_id,
        "disposition": incident.disposition.as_str(),
        "health": incident.health.as_str(),
        "socket": incident.socket
    })
}

fn startup_reconcile(context: &Context) -> Result<Reconciliation, Box<dyn std::error::Error>> {
    let sessions = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .list_sessions()?;
    let expected_control_sockets: HashSet<PathBuf> = sessions
        .iter()
        .map(|session| worker_socket(&context.runtime_dir, &session.id))
        .collect();
    let expected_attach_sockets: HashSet<PathBuf> = sessions
        .iter()
        .map(|session| worker_attach_socket(&context.runtime_dir, &session.id))
        .collect();
    let runtime_entries = fs::read_dir(&context.runtime_dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    matches!(
                        path.extension().and_then(|value| value.to_str()),
                        Some("sock" | "a")
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let control_probes: HashMap<PathBuf, Option<Value>> = runtime_entries
        .iter()
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("sock"))
        .map(|path| (path.clone(), probe_runtime_worker(context, path)))
        .collect();
    let mut healthy_groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for (path, status) in &control_probes {
        if let Some(session_id) = status
            .as_ref()
            .and_then(|status| status.get("session_id"))
            .and_then(Value::as_str)
        {
            healthy_groups
                .entry(session_id.to_string())
                .or_default()
                .push(path.clone());
        }
    }
    let duplicate_sessions: HashSet<String> = healthy_groups
        .iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(session_id, _)| session_id.clone())
        .collect();
    let mut report = Reconciliation::default();
    for (session_id, paths) in &healthy_groups {
        if paths.len() <= 1 {
            continue;
        }
        report.conflict_sessions += 1;
        for path in paths {
            report.incidents.push(RuntimeIncident {
                session_id: Some(session_id.clone()),
                disposition: ReconciliationDisposition::Conflict,
                health: WorkerHealth::Healthy,
                socket: path.clone(),
            });
            quarantine_runtime_family(path);
        }
    }
    for session in sessions {
        if duplicate_sessions.contains(&session.id) {
            continue;
        }
        if session.state == "closed" {
            let socket = worker_socket(&context.runtime_dir, &session.id);
            if socket.exists() {
                report.conflict_sessions += 1;
                report.incidents.push(RuntimeIncident {
                    session_id: Some(session.id.clone()),
                    disposition: ReconciliationDisposition::Conflict,
                    health: if control_probes.get(&socket).is_some_and(Option::is_some) {
                        WorkerHealth::Healthy
                    } else {
                        WorkerHealth::Unreachable
                    },
                    socket: socket.clone(),
                });
                quarantine_worker(context, &session.id);
            }
            continue;
        }
        let socket = worker_socket(&context.runtime_dir, &session.id);
        if !socket.exists() {
            let alternate = control_probes.iter().find_map(|(path, status)| {
                (path != &socket
                    && path.exists()
                    && status
                        .as_ref()
                        .and_then(|status| status.get("session_id"))
                        .and_then(Value::as_str)
                        == Some(session.id.as_str()))
                .then_some(path)
            });
            if let Some(path) = alternate {
                quarantine_runtime_family(path);
                mark_worker_lost(context, &session.id)?;
                report.conflict_sessions += 1;
                report.incidents.push(RuntimeIncident {
                    session_id: Some(session.id.clone()),
                    disposition: ReconciliationDisposition::Conflict,
                    health: WorkerHealth::Healthy,
                    socket: path.clone(),
                });
                continue;
            }
            if session
                .worker_identity
                .as_ref()
                .is_some_and(qwe_runtime::process_matches)
            {
                remember_unreachable(context, &session);
            } else {
                mark_worker_lost(context, &session.id)?;
            }
            continue;
        }
        match connect_existing_worker(context, &session.id, socket.clone()) {
            Ok((client, status)) => {
                let health_client = match connect_health_client(context, &session.id, &socket) {
                    Ok(client) => client,
                    Err(failure) => {
                        eprintln!(
                            "worker health channel unavailable for {}: {}",
                            session.id, failure
                        );
                        remember_unreachable(context, &session);
                        continue;
                    }
                };
                match worker_handle(
                    &session.id,
                    socket.clone(),
                    &status,
                    Some(client.clone()),
                    Some(health_client),
                    WorkerHealth::Healthy,
                ) {
                    Ok(handle) => {
                        if session
                            .worker_identity
                            .as_ref()
                            .is_some_and(|identity| identity != &handle.worker_identity)
                        {
                            quarantine_worker(context, &session.id);
                            mark_worker_lost(context, &session.id)?;
                            report.conflict_sessions += 1;
                            report.incidents.push(RuntimeIncident {
                                session_id: Some(session.id.clone()),
                                disposition: ReconciliationDisposition::Conflict,
                                health: WorkerHealth::Healthy,
                                socket: socket.clone(),
                            });
                            continue;
                        }
                        apply_status(context, &session.id, &status)
                            .map_err(|failure| format!("{}: {}", failure.code, failure.message))?;
                        context
                            .workers
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .insert(session.id.clone(), Arc::new(handle));
                    }
                    Err(failure) => {
                        quarantine_worker(context, &session.id);
                        mark_worker_lost(context, &session.id)?;
                        report.conflict_sessions += 1;
                        report.incidents.push(RuntimeIncident {
                            session_id: Some(session.id.clone()),
                            disposition: ReconciliationDisposition::Conflict,
                            health: WorkerHealth::Healthy,
                            socket: socket.clone(),
                        });
                        eprintln!(
                            "worker identity conflict for {}: {}",
                            session.id, failure.message
                        );
                    }
                }
            }
            Err(_) => {
                if session
                    .worker_identity
                    .as_ref()
                    .is_some_and(qwe_runtime::process_matches)
                {
                    remember_unreachable(context, &session);
                } else {
                    mark_worker_lost(context, &session.id)?;
                }
            }
        }
    }
    let mut orphan_ids = HashSet::new();
    for path in runtime_entries {
        if !path.exists()
            || expected_control_sockets.contains(&path)
            || expected_attach_sockets.contains(&path)
        {
            continue;
        }
        let probe = control_probes.get(&path).and_then(Option::as_ref);
        let session_id = probe
            .and_then(|status| status.get("session_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                path.file_stem()
                    .and_then(|value| value.to_str())
                    .and_then(hex_decode)
            });
        let health = if probe.is_some() {
            WorkerHealth::Healthy
        } else {
            WorkerHealth::Unreachable
        };
        let identity = session_id
            .clone()
            .unwrap_or_else(|| path.display().to_string());
        orphan_ids.insert(identity);
        report.incidents.push(RuntimeIncident {
            session_id,
            disposition: ReconciliationDisposition::Orphaned,
            health,
            socket: path,
        });
    }
    report.orphaned_sessions = orphan_ids.len();
    Ok(report)
}

fn reconnect_worker_handle(
    context: &Context,
    session_id: &str,
    previous: &WorkerHandle,
) -> Option<Arc<WorkerHandle>> {
    if !qwe_runtime::process_matches(&previous.worker_identity) || !previous.socket.exists() {
        return None;
    }
    let (client, status) =
        connect_existing_worker(context, session_id, previous.socket.clone()).ok()?;
    let health_client = connect_health_client(context, session_id, &previous.socket).ok()?;
    let handle = worker_handle(
        session_id,
        previous.socket.clone(),
        &status,
        Some(client),
        Some(health_client),
        WorkerHealth::Healthy,
    )
    .ok()?;
    if handle.worker_identity != previous.worker_identity {
        return None;
    }
    Some(Arc::new(handle))
}

fn safety_scan(context: &Context) {
    let handles: Vec<_> = context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|(id, handle)| (id.clone(), Arc::clone(handle)))
        .collect();
    for (session_id, handle) in handles {
        if !qwe_runtime::process_matches(&handle.worker_identity) {
            let _ = mark_worker_lost(context, &session_id);
            context
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&session_id);
            remove_worker_files(context, &session_id);
            continue;
        }
        let health_result = handle.health_client.as_ref().map_or_else(
            || Err(error("worker_unreachable", "health channel missing")),
            |client| client.call("PING", json!({}), WORKER_CALL_TIMEOUT),
        );
        match health_result {
            Ok(status) => {
                let main_closed = handle
                    .client
                    .as_ref()
                    .is_none_or(|client| client.is_closed());
                if main_closed {
                    if let Some(reconnected) =
                        reconnect_worker_handle(context, &session_id, &handle)
                    {
                        context
                            .workers
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .insert(session_id.clone(), reconnected);
                    } else {
                        set_worker_health(&handle, WorkerHealth::Unreachable);
                    }
                } else {
                    set_worker_health(&handle, WorkerHealth::Healthy);
                }
                let _ = apply_status(context, &session_id, &status);
            }
            Err(_) => {
                set_worker_health(&handle, WorkerHealth::Unreachable);
                if let Some(reconnected) = reconnect_worker_handle(context, &session_id, &handle) {
                    context
                        .workers
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .insert(session_id.clone(), reconnected);
                }
            }
        }
    }
    context.last_scan_ms.store(now_ms(), AtomicOrdering::SeqCst);
}

fn control_status(context: &Context) -> Result<Value, ProtocolErrorBody> {
    let store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let sessions = store.list_sessions().map_err(storage_error)?;
    let panes = store.list_panes().map_err(storage_error)?;
    let tasks = store.list_tasks().map_err(storage_error)?;
    let workers = context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|(id, handle)| worker_json(id, handle))
        .collect::<Vec<_>>();
    let healthy_workers = workers
        .iter()
        .filter(|worker| worker["health"].as_str() == Some("healthy"))
        .count();
    let unreachable_workers = workers
        .iter()
        .filter(|worker| worker["health"].as_str() == Some("unreachable"))
        .count();
    let incidents = context
        .reconciliation_incidents
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let orphaned_entries = incidents
        .incidents
        .iter()
        .filter(|incident| incident.disposition == ReconciliationDisposition::Orphaned)
        .map(runtime_incident_json)
        .collect::<Vec<_>>();
    let conflict_entries = incidents
        .incidents
        .iter()
        .filter(|incident| incident.disposition == ReconciliationDisposition::Conflict)
        .map(runtime_incident_json)
        .collect::<Vec<_>>();
    let credential_names = context.credentials.names();
    let credential_error = credential_names.as_ref().err().map(ToString::to_string);
    let credentials = credential_names.unwrap_or_default();
    Ok(json!({
        "daemon": "ok",
        "protocol": PROTOCOL_VERSION,
        "capabilities": capability_matrix(context.linux_scope_available),
        "credentials": {
            "configured": context.credentials.backend().as_str() != "none",
            "backend": context.credentials.backend().as_str(),
            "available": credential_error.is_none(),
            "error": credential_error,
            "names": credentials
        },
        "limits": {
            "control_frame_bytes": qwe_protocol::MAX_FRAME_BYTES,
            "control_input_bytes": MAX_CONTROL_INPUT_BYTES
        },
        "reconciliation": {
            "live_sessions": healthy_workers,
            "unreachable_sessions": unreachable_workers,
            "orphaned_sessions": incidents.orphaned_sessions,
            "conflict_sessions": incidents.conflict_sessions,
            "orphaned_runtime_entries": orphaned_entries,
            "conflict_runtime_entries": conflict_entries,
            "lost_sessions": sessions.iter().filter(|session| session.state == "lost").count(),
            "exited_tasks": tasks.iter().filter(|task| task.state == TaskState::Exited).count(),
            "lost_tasks": tasks.iter().filter(|task| task.state == TaskState::Lost).count(),
            "last_scan_ms": context.last_scan_ms.load(AtomicOrdering::SeqCst)
        },
        "workers": workers,
        "panes": panes,
        "sessions": sessions,
        "tasks": tasks
    }))
}

fn prune_history(context: &Context) {
    let report = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .prune_terminal_history(
            now_ms(),
            HISTORY_RETENTION_MS,
            HISTORY_MAX_TERMINAL_SESSIONS,
        );
    match report {
        Ok(report) if report.sessions > 0 => eprintln!(
            "history retention pruned {} Sessions, {} Tasks, and {} Panes",
            report.sessions, report.tasks, report.panes
        ),
        Ok(_) => {}
        Err(failure) => eprintln!("history retention failed: {failure}"),
    }
}

fn client_operation(
    context: &Context,
    request: &RequestEnvelope,
) -> Result<Value, ProtocolErrorBody> {
    match request.operation.as_str() {
        "PING" => Ok(json!({"daemon": "ok", "protocol": PROTOCOL_VERSION})),
        "CONTROL_STATUS" => control_status(context),
        "SESSION_CREATE" => create_session(context, request.payload.clone()),
        "TASK_STATUS" => task_status(context, &request.payload),
        "SESSION_RESOURCE_UPDATE" => update_session_resources(context, &request.payload),
        "TASK_STOP" => task_stop(context, &request.payload),
        "TASK_KILL_BATCH" => task_kill_batch(context, &request.payload),
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
        "SESSION_CLOSE" => close_session(context, &request.payload),
        "SESSION_KILL" => kill_session(context, &request.payload),
        "SERVER_KILL" => kill_server(context, &request.payload),
        "SESSION_INPUT" => {
            let session_id = required_string(&request.payload, "session_id")?;
            let input_bytes = request
                .payload
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| error("invalid_request", "missing input data"))?
                .len();
            if input_bytes > MAX_CONTROL_INPUT_BYTES {
                return Err(error(
                    "input_too_large",
                    format!(
                        "control input is {input_bytes} bytes; maximum is {MAX_CONTROL_INPUT_BYTES}"
                    ),
                ));
            }
            let _authority_guard = context
                .attachment_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            {
                let mut store = context
                    .store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                authorize_session_control(
                    &mut store,
                    session_id,
                    &request.payload,
                    AttachmentOperation::InputResize,
                    now_ms(),
                )?;
            }
            call_worker(
                context,
                session_id,
                &request.operation,
                request.payload.clone(),
                WORKER_REQUEST_TIMEOUT,
            )
        }
        "SESSION_STATUS" | "SESSION_ATTACH" => {
            let session_id = required_string(&request.payload, "session_id")?;
            let status = match call_worker(
                context,
                session_id,
                &request.operation,
                request.payload.clone(),
                WORKER_REQUEST_TIMEOUT,
            ) {
                Ok(status) => status,
                Err(failure)
                    if request.operation == "SESSION_STATUS" && failure.code == "terminal_lost" =>
                {
                    return match persisted_session_status(context, session_id) {
                        Ok(status)
                            if status.get("state").and_then(Value::as_str) == Some("closed") =>
                        {
                            Ok(status)
                        }
                        Err(persisted) if persisted.code == "not_found" => Err(failure),
                        Err(persisted) if persisted.code == "persistence_failure" => Err(persisted),
                        _ => Err(failure),
                    };
                }
                Err(failure) => return Err(failure),
            };
            apply_status(context, session_id, &status)?;
            Ok(status)
        }
        _ => Err(error("unsupported_operation", request.operation.clone())),
    }
}

fn requires_idempotency(operation: &str) -> bool {
    matches!(
        operation,
        "SESSION_CREATE"
            | "SESSION_DETACH"
            | "SESSION_INPUT"
            | "SESSION_RESOURCE_UPDATE"
            | "SESSION_RESIZE"
            | "SESSION_CLOSE"
            | "SESSION_KILL"
            | "SERVER_KILL"
            | "TASK_STOP"
            | "TASK_KILL_BATCH"
            | "LEASE_CLAIM"
            | "LEASE_TOUCH"
            | "LEASE_RELEASE"
            | "LEASE_TAKEOVER"
    )
}

fn request_fingerprint(request: &RequestEnvelope) -> Result<String, ProtocolErrorBody> {
    let encoded = serde_json::to_vec(&(&request.operation, &request.payload))
        .map_err(|failure| error("invalid_request", failure.to_string()))?;
    let digest = Sha256::digest(encoded);
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(fingerprint, "{byte:02x}");
    }
    Ok(fingerprint)
}

fn replayable_result(result: &Result<Value, ProtocolErrorBody>) -> bool {
    match result {
        Ok(_) => true,
        Err(failure) => matches!(
            failure.code.as_str(),
            "already_attached"
                | "already_exists"
                | "confirmation_required"
                | "credential_unavailable"
                | "input_too_large"
                | "invalid_reclaim_token"
                | "invalid_request"
                | "not_attached"
                | "not_found"
                | "not_owner"
                | "reclaim_expired"
                | "unsupported_mode"
                | "unsupported_capability"
                | "unsupported_operation"
        ),
    }
}

fn recover_pending_create(
    context: &Context,
    payload: &Value,
) -> Result<Option<Value>, ProtocolErrorBody> {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error("invalid_request", "missing session_id"))?;
    let persisted = {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session) = store.find_session(session_id).map_err(storage_error)? else {
            return Ok(None);
        };
        let pane = store
            .find_pane_for_session(session_id)
            .map_err(storage_error)?
            .ok_or_else(|| error("persistence_failure", "created Pane is missing"))?;
        let task = store
            .find_task_for_session(session_id)
            .map_err(storage_error)?
            .ok_or_else(|| error("persistence_failure", "created TaskInstance is missing"))?;
        (session, pane, task)
    };
    let (session, pane, task) = persisted;
    let mut response = call_worker(
        context,
        session_id,
        "SESSION_STATUS",
        json!({"session_id": session_id}),
        WORKER_REQUEST_TIMEOUT,
    )
    .map_err(|failure| {
        error(
            "idempotency_indeterminate",
            format!(
                "creation completed but the worker status could not be recovered: {}",
                failure.message
            ),
        )
    })?;
    response["task_id"] = json!(task.id);
    response["pane_id"] = json!(pane.id);
    response["resource_policy"] = json!(pane.resource_policy);
    response["running"] = json!(session.state == "running");
    response["state"] = json!(session.state);
    response["exit_code"] = json!(task.exit_code);
    response["exit_signal"] = json!(task.exit_signal);
    if let Some(response) = response.as_object_mut() {
        response.remove("total_written");
        response.remove("truncated_from");
    }
    Ok(Some(response))
}

fn execute_request(
    context: &Context,
    request: &RequestEnvelope,
) -> Result<Value, ProtocolErrorBody> {
    let Some(key) = request
        .idempotency_key
        .as_deref()
        .filter(|key| !key.is_empty())
    else {
        if requires_idempotency(&request.operation) {
            return Err(error(
                "invalid_request",
                "mutating operations require an idempotency_key",
            ));
        }
        return client_operation(context, request);
    };
    if key.len() > 256 {
        return Err(error(
            "invalid_request",
            "idempotency_key must be at most 256 bytes",
        ));
    }
    let fingerprint = request_fingerprint(request)?;
    let _request = context
        .idempotency_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = now_ms();
    let existing = {
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        store
            .prune_expired_idempotency(now)
            .map_err(storage_error)?;
        store.find_idempotency(key, now).map_err(storage_error)?
    };
    if let Some(record) = existing {
        if record.operation != request.operation || record.request_fingerprint != fingerprint {
            return Err(error(
                "idempotency_conflict",
                "idempotency_key was already used for a different request",
            ));
        }
        if record.state == IDEMPOTENCY_COMPLETE {
            let response_json = record.response_json.ok_or_else(|| {
                error(
                    "persistence_failure",
                    "completed idempotency record has no response",
                )
            })?;
            let stored: Result<Value, ProtocolErrorBody> = serde_json::from_str(&response_json)
                .map_err(|failure| error("persistence_failure", failure.to_string()))?;
            return stored;
        }
        if record.state == qwe_storage::IDEMPOTENCY_PENDING && request.operation == "SESSION_CREATE"
        {
            if let Some(response) = recover_pending_create(context, &request.payload)? {
                let serialized =
                    serde_json::to_string(&Ok::<Value, ProtocolErrorBody>(response.clone()))
                        .map_err(|failure| error("persistence_failure", failure.to_string()))?;
                context
                    .store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .complete_idempotency(key, &serialized)
                    .map_err(storage_error)?;
                return Ok(response);
            }
            context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .delete_idempotency(key)
                .map_err(storage_error)?;
        } else {
            return Err(error(
                "idempotency_indeterminate",
                "the prior request may have taken effect; automatic replay is not available",
            ));
        }
    }

    let reserved = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .reserve_idempotency(key, &request.operation, &fingerprint, now)
        .map_err(storage_error)?;
    if !reserved {
        return Err(error(
            "idempotency_in_progress",
            "the same idempotency key is already in progress",
        ));
    }

    let result = client_operation(context, request);
    if !replayable_result(&result) {
        context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .delete_idempotency(key)
            .map_err(storage_error)?;
        return result;
    }
    let serialized = serde_json::to_string(&result)
        .map_err(|failure| error("persistence_failure", failure.to_string()))?;
    context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .complete_idempotency(key, &serialized)
        .map_err(storage_error)?;
    result
}

fn proxy_control(
    mut client: UnixStream,
    context: &Context,
) -> Result<(), Box<dyn std::error::Error>> {
    verify_same_uid(&client)?;
    let request: RequestEnvelope = read_frame(&mut client)?;
    validate_version(request.version)
        .map_err(|failure| std::io::Error::other(format!("incompatible_version: {failure}")))?;
    let response = ResponseEnvelope {
        version: PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        result: execute_request(context, &request),
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

fn secure_attach_token() -> Result<String, attach::AttachError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|failure| attach_error("internal", format!("random source failed: {failure}")))?;
    Ok(hex_encode(&bytes))
}

fn send_attachment_error(stream: &mut UnixStream, failure: attach::AttachError) {
    let _ = attach::write_json_frame(stream, attach::FrameType::Error, &failure);
}

fn forward_attachment_frame(
    stream: &mut UnixStream,
    frame: &attach::ReceivedFrame,
) -> Result<(), attach::AttachIoError> {
    attach::write_payload_frame(stream, frame.frame_type, &frame.payload)
}

enum ClientAttachmentEnd {
    Eof,
    Detach(i64),
    Error(attach::AttachError),
}

fn serve_client_attachment_input(
    mut client: UnixStream,
    mut worker: UnixStream,
) -> ClientAttachmentEnd {
    loop {
        let frame = match attach::read_frame(&mut client) {
            Ok(frame) => frame,
            Err(attach::AttachIoError::Codec(failure)) => {
                let _ = worker.shutdown(std::net::Shutdown::Both);
                return ClientAttachmentEnd::Error(attach_error(
                    "unknown_frame_type",
                    failure.to_string(),
                ));
            }
            Err(_) => {
                let _ = worker.shutdown(std::net::Shutdown::Both);
                return ClientAttachmentEnd::Eof;
            }
        };
        if frame.frame_type == attach::FrameType::Detach {
            let detach = match attach::decode_json::<attach::Detach>(&frame.payload) {
                Ok(detach) => detach,
                Err(failure) => {
                    let _ = worker.shutdown(std::net::Shutdown::Both);
                    return ClientAttachmentEnd::Error(attach_error(
                        "worker_protocol_error",
                        failure.to_string(),
                    ));
                }
            };
            if detach.protocol_version != ATTACH_PROTOCOL_VERSION {
                let _ = worker.shutdown(std::net::Shutdown::Both);
                return ClientAttachmentEnd::Error(attach_error(
                    "incompatible_version",
                    format!(
                        "attach protocol {} is incompatible with {}",
                        detach.protocol_version, ATTACH_PROTOCOL_VERSION
                    ),
                ));
            }
            let Ok(hold_ttl_ms) = i64::try_from(detach.hold_ttl_ms) else {
                let _ = worker.shutdown(std::net::Shutdown::Both);
                return ClientAttachmentEnd::Error(attach_error(
                    "invalid_request",
                    "detached hold TTL is too large",
                ));
            };
            if hold_ttl_ms > MAX_LEASE_TTL_MS {
                let _ = worker.shutdown(std::net::Shutdown::Both);
                return ClientAttachmentEnd::Error(attach_error(
                    "invalid_request",
                    format!("detached hold TTL must not exceed {MAX_LEASE_TTL_MS} ms"),
                ));
            }
            let _ = worker.shutdown(std::net::Shutdown::Both);
            return ClientAttachmentEnd::Detach(hold_ttl_ms);
        }
        if let Err(failure) = forward_attachment_frame(&mut worker, &frame) {
            let _ = worker.shutdown(std::net::Shutdown::Both);
            return if matches!(failure, attach::AttachIoError::Codec(_)) {
                ClientAttachmentEnd::Error(attach_error("unknown_frame_type", failure.to_string()))
            } else {
                ClientAttachmentEnd::Eof
            };
        }
    }
}

fn serve_attachment(
    mut client: UnixStream,
    context: &Arc<Context>,
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
    if open.rows == 0 || open.columns == 0 {
        send_attachment_error(
            &mut client,
            attach_error("invalid_request", "terminal dimensions must be positive"),
        );
        return Ok(());
    }

    let (raw_token, generation, worker) = {
        let _authority_guard = context
            .attachment_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = now_ms();
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session) = store
            .find_session(&open.session_id)
            .map_err(|failure| std::io::Error::other(failure.to_string()))?
        else {
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error("session_lost", "session does not exist"),
            );
            return Ok(());
        };
        if matches!(session.state.as_str(), "lost" | "closed") {
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error("session_lost", "session is no longer live"),
            );
            return Ok(());
        }
        let mut authority = store
            .get_attachment_authority(&open.session_id)
            .map_err(|failure| std::io::Error::other(failure.to_string()))?
            .unwrap_or_else(AttachmentAuthority::unattached);
        if authority.state != authority.state_at(now) {
            authority.release();
            store
                .write_attachment_authority(&open.session_id, &authority, now)
                .map_err(|failure| std::io::Error::other(failure.to_string()))?;
        }
        if let Err(failure) = authority.authorize(
            AttachmentOperation::Attach,
            AuthorityCredentials {
                owner: Some(&open.owner),
                reclaim_token: open.reclaim_token.as_deref(),
                lease_generation: open.lease_generation,
            },
            now,
        ) {
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error(failure.code(), failure.to_string()),
            );
            return Ok(());
        }
        let release_hold = if authority.state == AttachmentAuthorityState::Held {
            Some((
                authority.owner.clone().unwrap_or_default(),
                authority.generation,
            ))
        } else {
            None
        };

        let handle = context
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&open.session_id)
            .cloned();
        let Some(handle) = handle else {
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error("session_lost", "session has no live worker"),
            );
            return Ok(());
        };
        if worker_health(&handle) == WorkerHealth::Unreachable {
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error("worker_unreachable", "session worker is not responding"),
            );
            return Ok(());
        }
        let mut worker = match UnixStream::connect(&handle.attach_socket) {
            Ok(worker) => worker,
            Err(failure) => {
                drop(store);
                send_attachment_error(
                    &mut client,
                    attach_error(
                        "worker_unreachable",
                        format!("{}: {failure}", handle.attach_socket.display()),
                    ),
                );
                return Ok(());
            }
        };
        verify_same_uid(&worker)
            .map_err(|failure| format!("worker attach peer verification failed: {failure}"))?;
        let internal_open = attach::AttachOpen::versioned(
            &open.session_id,
            secure_attach_token().map_err(|failure| std::io::Error::other(failure.message))?,
            open.rows,
            open.columns,
        );
        if let Err(failure) =
            attach::write_json_frame(&mut worker, attach::FrameType::AttachOpen, &internal_open)
        {
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error("worker_protocol_error", failure.to_string()),
            );
            return Ok(());
        }
        let worker_accepted = match attach::read_frame(&mut worker) {
            Ok(response) if response.frame_type == attach::FrameType::AttachAccepted => response,
            Ok(response) if response.frame_type == attach::FrameType::Error => {
                drop(store);
                let _ = forward_attachment_frame(&mut client, &response);
                return Ok(());
            }
            Ok(response) => {
                drop(store);
                send_attachment_error(
                    &mut client,
                    attach_error(
                        "worker_protocol_error",
                        format!(
                            "expected ATTACH_ACCEPTED, received {}",
                            response.frame_type.name()
                        ),
                    ),
                );
                return Ok(());
            }
            Err(failure) => {
                drop(store);
                send_attachment_error(
                    &mut client,
                    attach_error("worker_protocol_error", failure.to_string()),
                );
                return Ok(());
            }
        };
        let worker_accepted =
            match attach::decode_json::<attach::AttachAccepted>(&worker_accepted.payload) {
                Ok(accepted) => accepted,
                Err(failure) => {
                    drop(store);
                    send_attachment_error(
                        &mut client,
                        attach_error("worker_protocol_error", failure.to_string()),
                    );
                    return Ok(());
                }
            };
        let snapshot_frame = match attach::read_frame(&mut worker) {
            Ok(response) if response.frame_type == attach::FrameType::Snapshot => response,
            Ok(response) => {
                let _ = worker.shutdown(std::net::Shutdown::Both);
                drop(store);
                send_attachment_error(
                    &mut client,
                    attach_error(
                        "worker_protocol_error",
                        format!("expected SNAPSHOT, received {}", response.frame_type.name()),
                    ),
                );
                return Ok(());
            }
            Err(failure) => {
                let _ = worker.shutdown(std::net::Shutdown::Both);
                drop(store);
                send_attachment_error(
                    &mut client,
                    attach_error("worker_protocol_error", failure.to_string()),
                );
                return Ok(());
            }
        };
        let snapshot =
            match attach::decode_json::<attach::TerminalSnapshot>(&snapshot_frame.payload) {
                Ok(snapshot) => snapshot,
                Err(failure) => {
                    let _ = worker.shutdown(std::net::Shutdown::Both);
                    drop(store);
                    send_attachment_error(
                        &mut client,
                        attach_error("worker_protocol_error", failure.to_string()),
                    );
                    return Ok(());
                }
            };

        let raw_token = match secure_attach_token() {
            Ok(token) => token,
            Err(failure) => {
                let _ = worker.shutdown(std::net::Shutdown::Both);
                drop(store);
                send_attachment_error(&mut client, failure);
                return Ok(());
            }
        };
        let replacement_hash = ReclaimTokenHash::from_token(&raw_token);
        let mut next = authority;
        let transition = match next.state {
            AttachmentAuthorityState::Unattached => {
                next.grant_active(&open.owner, replacement_hash)
            }
            AttachmentAuthorityState::ReclaimGrace => next.attach_from_reclaim(
                open.reclaim_token.as_deref().unwrap_or_default(),
                replacement_hash,
                now,
            ),
            AttachmentAuthorityState::Held => next.attach_from_hold(
                &open.owner,
                open.lease_generation.unwrap_or_default(),
                replacement_hash,
                now,
            ),
            AttachmentAuthorityState::Active => unreachable!("active attach was rejected"),
        };
        if let Err(failure) = transition {
            let _ = worker.shutdown(std::net::Shutdown::Both);
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error(failure.code(), failure.to_string()),
            );
            return Ok(());
        }
        if let Err(failure) = store.activate_attachment(
            &open.session_id,
            &next,
            release_hold
                .as_ref()
                .map(|(owner, generation)| (owner.as_str(), *generation)),
            now,
        ) {
            let _ = worker.shutdown(std::net::Shutdown::Both);
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error("persistence_failure", failure.to_string()),
            );
            return Ok(());
        }
        let accepted = attach::AttachAccepted {
            protocol_version: ATTACH_PROTOCOL_VERSION,
            session_id: open.session_id.clone(),
            owner: open.owner.clone(),
            generation: next.generation,
            reclaim_token: raw_token.clone(),
            rows: worker_accepted.rows,
            columns: worker_accepted.columns,
            stream_offset: snapshot.next_stream_offset,
        };
        if let Err(failure) =
            attach::write_json_frame(&mut client, attach::FrameType::AttachAccepted, &accepted)
                .and_then(|()| {
                    attach::write_json_frame(&mut client, attach::FrameType::Snapshot, &snapshot)
                })
        {
            let _ = worker.shutdown(std::net::Shutdown::Both);
            let mut released = next;
            released.release();
            let _ = store.write_attachment_authority(&open.session_id, &released, now_ms());
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error("worker_protocol_error", failure.to_string()),
            );
            return Ok(());
        }
        drop(store);
        (raw_token, next.generation, worker)
    };

    let input_client = client.try_clone()?;
    let input_worker = worker.try_clone()?;
    let input = thread::spawn(move || serve_client_attachment_input(input_client, input_worker));
    let mut worker_reader = worker;
    while let Ok(frame) = attach::read_frame(&mut worker_reader) {
        if forward_attachment_frame(&mut client, &frame).is_err() {
            break;
        }
    }
    let _ = client.shutdown(std::net::Shutdown::Read);
    let end = input.join().unwrap_or_else(|_| {
        ClientAttachmentEnd::Error(attach_error(
            "internal",
            "attachment input thread stopped unexpectedly",
        ))
    });
    if let ClientAttachmentEnd::Error(failure) = &end {
        send_attachment_error(&mut client, failure.clone());
    }

    let _authority_guard = context
        .attachment_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = now_ms();
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(mut authority) = store
        .get_attachment_authority(&open.session_id)
        .map_err(|failure| std::io::Error::other(failure.to_string()))?
    {
        if authority.state == AttachmentAuthorityState::Active
            && authority.owner.as_deref() == Some(&open.owner)
            && authority
                .reclaim_token_hash
                .as_ref()
                .is_some_and(|hash| hash.matches(&raw_token))
            && authority.generation == generation
        {
            let hold_ttl_ms = match end {
                ClientAttachmentEnd::Detach(hold_ttl_ms) => hold_ttl_ms,
                ClientAttachmentEnd::Eof | ClientAttachmentEnd::Error(_) => 0,
            };
            authority.detach(hold_ttl_ms, now).map_err(|failure| {
                std::io::Error::other(format!("attachment transition failed: {failure}"))
            })?;
            if authority.state == AttachmentAuthorityState::Held {
                store
                    .persist_attachment_hold(
                        &open.session_id,
                        authority.owner.as_deref().unwrap_or_default(),
                        authority.generation,
                        authority.deadline_ms.unwrap_or(now),
                        now,
                    )
                    .map_err(|failure| std::io::Error::other(failure.to_string()))?;
            } else {
                store
                    .write_attachment_authority(&open.session_id, &authority, now)
                    .map_err(|failure| std::io::Error::other(failure.to_string()))?;
            }
        }
    }
    drop(store);
    let _ = client.shutdown(std::net::Shutdown::Both);
    Ok(())
}

extern "C" fn sigchld_handler(_: libc::c_int) {
    let fd = SIGCHLD_WRITE_FD.load(AtomicOrdering::Relaxed);
    if fd < 0 {
        return;
    }
    let byte = 1_u8;
    unsafe {
        let _ = libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
    }
}

extern "C" fn shutdown_handler(_: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, AtomicOrdering::SeqCst);
}

fn install_shutdown_handler() -> Result<(), Box<dyn std::error::Error>> {
    let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
    action.sa_sigaction = shutdown_handler as *const () as usize;
    action.sa_flags = 0;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
    }
    for signal in [libc::SIGTERM, libc::SIGINT] {
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}

fn install_sigchld_monitor(
    event_sender: Sender<SupervisorEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut pipe_fds = [0_i32; 2];
    if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    for fd in pipe_fds {
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    unsafe {
        libc::fcntl(pipe_fds[1], libc::F_SETFL, libc::O_NONBLOCK);
    }
    SIGCHLD_WRITE_FD.store(pipe_fds[1], AtomicOrdering::Relaxed);
    let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
    action.sa_sigaction = sigchld_handler as *const () as usize;
    action.sa_flags = libc::SA_RESTART;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
    }
    if unsafe { libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut()) } != 0 {
        SIGCHLD_WRITE_FD.store(-1, AtomicOrdering::Relaxed);
        return Err(std::io::Error::last_os_error().into());
    }
    thread::spawn(move || {
        let mut buffer = [0_u8; 64];
        loop {
            let count = unsafe {
                libc::read(
                    pipe_fds[0],
                    buffer.as_mut_ptr().cast::<libc::c_void>(),
                    buffer.len(),
                )
            };
            if count < 0 {
                let failure = std::io::Error::last_os_error();
                if failure.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            if count == 0 {
                return;
            }
            loop {
                let mut status = 0;
                let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
                if pid <= 0 {
                    break;
                }
                let _ = event_sender.send(SupervisorEvent::ChildExited { pid: pid as u32 });
            }
        }
    });
    Ok(())
}

fn start_monitor_threads(
    context: &Arc<Context>,
    events: Receiver<SupervisorEvent>,
    event_sender: Sender<SupervisorEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let monitor_context = Arc::clone(context);
    thread::spawn(move || {
        for event in events {
            match event {
                SupervisorEvent::Worker(WorkerEvent::ControlClosed { session_id }) => {
                    let handle = monitor_context
                        .workers
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .get(&session_id)
                        .cloned();
                    if let Some(handle) = handle {
                        set_worker_health(&handle, WorkerHealth::Unreachable);
                    }
                }
                SupervisorEvent::ChildExited { pid } => {
                    let session_id = monitor_context
                        .workers
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .iter()
                        .find(|(_, handle)| handle.worker_pid == pid)
                        .map(|(session_id, _)| session_id.clone());
                    if let Some(session_id) = session_id {
                        let _ = mark_worker_lost(&monitor_context, &session_id);
                        monitor_context
                            .workers
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .remove(&session_id);
                        remove_worker_files(&monitor_context, &session_id);
                    }
                }
            }
        }
    });

    install_sigchld_monitor(event_sender)?;

    let scan_context = Arc::clone(context);
    thread::spawn(move || {
        loop {
            thread::sleep(SAFETY_SCAN_INTERVAL);
            safety_scan(&scan_context);
        }
    });
    let history_context = Arc::clone(context);
    thread::spawn(move || {
        loop {
            thread::sleep(HISTORY_PRUNE_INTERVAL);
            prune_history(&history_context);
        }
    });
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    install_shutdown_handler()?;
    let linux_scope_available = qwe_runtime::systemd_scope_available();
    let runtime_dir = PathBuf::from(format!("{}.runtime", args.socket));
    let runtime_lock = acquire_runtime_lock(&runtime_dir)?;
    mark_subreaper();
    let store_path = args
        .store
        .unwrap_or_else(|| format!("{}.sqlite", args.socket));
    let store = Store::open(&store_path)?;
    let credentials_dir = args
        .credentials_dir
        .or_else(|| std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from));
    let keychain_prefix = args
        .keychain_service_prefix
        .or_else(|| std::env::var("QWE_KEYCHAIN_SERVICE_PREFIX").ok())
        .filter(|_| cfg!(target_os = "macos"));
    let keychain_path = args
        .keychain_path
        .or_else(|| std::env::var_os("QWE_KEYCHAIN_PATH").map(PathBuf::from))
        .filter(|_| cfg!(target_os = "macos"));
    let (worker_event_tx, worker_event_rx) = mpsc::channel();
    let (supervisor_event_tx, supervisor_event_rx) = mpsc::channel();
    let bridge_sender = supervisor_event_tx.clone();
    thread::spawn(move || {
        for event in worker_event_rx {
            if bridge_sender.send(SupervisorEvent::Worker(event)).is_err() {
                return;
            }
        }
    });
    let context = Arc::new(Context {
        runtime_dir,
        linux_scope_available,
        store: Mutex::new(store),
        workers: Mutex::new(HashMap::new()),
        reconciliation_incidents: Mutex::new(ReconciliationIncidents::default()),
        last_scan_ms: AtomicI64::new(0),
        worker_events: worker_event_tx,
        creation_lock: Mutex::new(()),
        idempotency_lock: Mutex::new(()),
        attachment_lock: Mutex::new(()),
        credentials: CredentialProvider::new(credentials_dir, keychain_prefix, keychain_path),
        _runtime_lock: runtime_lock,
    });
    let report = startup_reconcile(&context)?;
    context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .enter_reclaim_grace(now_ms(), DEFAULT_RECLAIM_GRACE_MS)?;
    prune_history(&context);
    *context
        .reconciliation_incidents
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = ReconciliationIncidents {
        orphaned_sessions: report.orphaned_sessions,
        conflict_sessions: report.conflict_sessions,
        incidents: report.incidents,
    };
    context.last_scan_ms.store(now_ms(), AtomicOrdering::SeqCst);

    let attach_path = public_attach_socket(&args.socket);
    let _ = fs::remove_file(&args.socket);
    let _ = fs::remove_file(&attach_path);
    let listener = UnixListener::bind(&args.socket)?;
    fs::set_permissions(&args.socket, fs::Permissions::from_mode(0o600))?;
    let attach_listener = UnixListener::bind(&attach_path)?;
    fs::set_permissions(&attach_path, fs::Permissions::from_mode(0o600))?;
    attach_listener.set_nonblocking(true)?;

    start_monitor_threads(&context, supervisor_event_rx, supervisor_event_tx)?;

    let control_context = Arc::clone(&context);
    thread::spawn(move || {
        for client in listener.incoming() {
            let context = Arc::clone(&control_context);
            thread::spawn(move || {
                if let Err(failure) = proxy_control(client?, &context) {
                    eprintln!("control protocol error: {failure}");
                }
                Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
            });
        }
    });
    while !SHUTDOWN_REQUESTED.load(AtomicOrdering::SeqCst) {
        match attach_listener.accept() {
            Ok((client, _)) => {
                client.set_nonblocking(false)?;
                let context = Arc::clone(&context);
                thread::spawn(move || {
                    if let Err(failure) = serve_attachment(client, &context) {
                        eprintln!("attach proxy error: {failure}");
                    }
                });
            }
            Err(failure) if failure.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(failure) if failure.kind() == std::io::ErrorKind::Interrupted => {}
            Err(failure) => return Err(failure.into()),
        }
    }
    graceful_shutdown(&context, &args.socket);
    Ok(())
}
