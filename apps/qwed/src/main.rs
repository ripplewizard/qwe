mod capabilities;
mod credentials;
mod security;
mod worker_client;

use capabilities::{matrix as capability_matrix, merge_resource_policy, resolve_resource_policy};
use clap::Parser;
use credentials::CredentialProvider;
use qwe_core::{
    AGENT_MAX_WAIT_MS, AgentCommandId, AgentCommandRecord, AgentCommandState, AgentOutputMode,
    AgentSignal, AttachmentAuthority, AttachmentAuthorityState, AttachmentOperation,
    AuthorityCredentials, CapabilityOperation, CapabilityScope, DEFAULT_CAPABILITY_TTL_MS,
    DEFAULT_FREEZE_GRACE_MS, DEFAULT_RECLAIM_GRACE_MS, LeaseRecord, MAX_FREEZE_GRACE_MS,
    MAX_LEASE_TTL_MS, PaneId, PaneKind, PaneRecord, PaneSpec, PaneWorkerRecord, PrincipalId,
    ProcessIdentity, ReclaimTokenHash, ReconciliationDisposition, ResourcePolicy, ResourceProfile,
    ResourceScopeId, RevocationSeverity, SandboxAuditRecord, SandboxProfile, SandboxRiskRecord,
    SessionRecord, TaskId, TaskRecord, TaskState, WorkerHealth, tile_pane_specs,
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
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use worker_client::{WorkerClient, WorkerEvent};
use zeroize::Zeroizing;

const WORKER_CALL_TIMEOUT: Duration = Duration::from_millis(500);
const WORKER_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const WORKER_AGENT_WAIT_TIMEOUT: Duration = Duration::from_secs(32);
const WORKER_MAINTENANCE_TIMEOUT: Duration = Duration::from_secs(5);
const SANDBOX_PROBE_TIMEOUT: Duration = Duration::from_secs(6);
const SAFETY_SCAN_INTERVAL: Duration = Duration::from_secs(2);
const SECURITY_RECONCILE_INTERVAL: Duration = Duration::from_millis(250);
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
    resource_support: qwe_runtime::ResourceSupport,
    store: Mutex<Store>,
    workers: Mutex<HashMap<String, Arc<WorkerHandle>>>,
    attachment_hubs: Mutex<HashMap<String, Arc<AttachmentHub>>>,
    reconciliation_incidents: Mutex<ReconciliationIncidents>,
    last_scan_ms: AtomicI64,
    worker_events: Sender<WorkerEvent>,
    creation_lock: Mutex<()>,
    idempotency_lock: Mutex<()>,
    attachment_lock: Mutex<()>,
    security_lock: Mutex<()>,
    credentials: CredentialProvider,
    _runtime_lock: File,
}

struct WorkerHandle {
    socket: PathBuf,
    attach_socket: PathBuf,
    pane_id: String,
    session_id: String,
    worker_pid: u32,
    worker_identity: ProcessIdentity,
    client: Option<Arc<WorkerClient>>,
    health_client: Option<Arc<WorkerClient>>,
    health: Mutex<WorkerHealth>,
}

struct WorkerSpawnSpec<'a> {
    session_id: &'a str,
    pane_id: &'a PaneId,
    program: &'a str,
    arguments: &'a [String],
    resource_policy: &'a ResourcePolicy,
    credential: Option<(&'a str, &'a Zeroizing<Vec<u8>>)>,
    sandbox_profile: Option<&'a SandboxProfile>,
}

struct AttachmentHub {
    next_observer_id: AtomicU64,
    observers: Mutex<HashMap<u64, mpsc::SyncSender<attach::ReceivedFrame>>>,
}

impl AttachmentHub {
    fn new() -> Self {
        Self {
            next_observer_id: AtomicU64::new(1),
            observers: Mutex::new(HashMap::new()),
        }
    }

    fn subscribe(&self) -> (u64, mpsc::Receiver<attach::ReceivedFrame>) {
        let id = self.next_observer_id.fetch_add(1, AtomicOrdering::SeqCst);
        let (sender, receiver) = mpsc::sync_channel(128);
        self.observers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, sender);
        (id, receiver)
    }

    fn unsubscribe(&self, id: u64) {
        self.observers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id);
    }

    fn broadcast(&self, frame: &attach::ReceivedFrame) {
        let mut observers = self
            .observers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        observers.retain(|_, sender| sender.try_send(frame.clone()).is_ok());
    }

    fn close_observers(&self) {
        self.observers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
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
    pane_id: Option<String>,
    disposition: ReconciliationDisposition,
    health: WorkerHealth,
    socket: PathBuf,
}

fn error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
        details: None,
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

fn worker_socket(runtime_dir: &Path, pane_id: &str) -> PathBuf {
    runtime_dir.join(format!("p{}.sock", compact_pane_key(pane_id)))
}

fn worker_attach_socket(runtime_dir: &Path, pane_id: &str) -> PathBuf {
    runtime_dir.join(format!("p{}.a", compact_pane_key(pane_id)))
}

fn compact_pane_key(pane_id: &str) -> String {
    pane_id
        .chars()
        .filter(|value| value.is_ascii_hexdigit())
        .collect()
}

fn legacy_worker_socket(runtime_dir: &Path, session_id: &str) -> PathBuf {
    runtime_dir.join(format!("{}.sock", hex_encode(session_id.as_bytes())))
}

fn legacy_worker_attach_socket(runtime_dir: &Path, session_id: &str) -> PathBuf {
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
    pane_id: &str,
    expected_socket: &Path,
    status: &Value,
) -> Result<(u32, ProcessIdentity), ProtocolErrorBody> {
    if status.get("session_id").and_then(Value::as_str) != Some(session_id) {
        return Err(error(
            "worker_contract",
            "worker reported the wrong SessionId",
        ));
    }
    if let Some(reported_pane) = status.get("pane_id").and_then(Value::as_str) {
        if reported_pane != pane_id {
            return Err(error("worker_contract", "worker reported the wrong PaneId"));
        }
    }
    let (worker_pid, identity) = parse_worker_identity(status)?;
    let runtime_dir = expected_socket
        .parent()
        .expect("worker socket has a runtime parent");
    let expected_pane_socket = worker_socket(runtime_dir, pane_id);
    let expected_legacy_socket = legacy_worker_socket(runtime_dir, session_id);
    if expected_socket != expected_pane_socket && expected_socket != expected_legacy_socket {
        return Err(error(
            "worker_contract",
            "worker socket path does not match the Pane or legacy Session",
        ));
    }
    let actual_socket = worker_socket(
        expected_socket
            .parent()
            .expect("worker socket has a runtime parent"),
        pane_id,
    );
    if actual_socket != expected_socket && expected_socket != expected_legacy_socket {
        return Err(error(
            "worker_contract",
            "worker socket path does not match the Pane",
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
    pane_id: &str,
    socket: PathBuf,
    status: &Value,
    client: Option<Arc<WorkerClient>>,
    health_client: Option<Arc<WorkerClient>>,
    health: WorkerHealth,
) -> Result<WorkerHandle, ProtocolErrorBody> {
    let (worker_pid, worker_identity) =
        validate_worker_status(session_id, pane_id, &socket, status)?;
    let reported_pane = status
        .get("pane_id")
        .and_then(Value::as_str)
        .unwrap_or(pane_id);
    let attach_socket = if socket
        == legacy_worker_socket(
            socket.parent().expect("worker socket has a runtime parent"),
            session_id,
        ) {
        legacy_worker_attach_socket(
            socket.parent().expect("worker socket has a runtime parent"),
            session_id,
        )
    } else {
        worker_attach_socket(
            socket.parent().expect("worker socket has a runtime parent"),
            reported_pane,
        )
    };
    Ok(WorkerHandle {
        pane_id: reported_pane.to_string(),
        session_id: session_id.to_string(),
        attach_socket,
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

fn worker_json(handle: &WorkerHandle) -> Value {
    json!({
        "session_id": handle.session_id,
        "pane_id": handle.pane_id,
        "pid": handle.worker_pid,
        "identity": handle.worker_identity,
        "health": worker_health(handle).as_str()
    })
}

fn cleanup_worker_scope(handle: &WorkerHandle) {
    let _ = qwe_runtime::stop_pane_scope(&handle.pane_id);
}

fn unreachable_worker_handle(
    context: &Context,
    session_id: &str,
    pane_id: &str,
    worker_identity: ProcessIdentity,
) -> Option<Arc<WorkerHandle>> {
    let socket = worker_socket(&context.runtime_dir, pane_id);
    Some(Arc::new(WorkerHandle {
        pane_id: pane_id.to_string(),
        session_id: session_id.to_string(),
        attach_socket: worker_attach_socket(&context.runtime_dir, pane_id),
        worker_pid: worker_identity.pid,
        worker_identity,
        socket,
        client: None,
        health_client: None,
        health: Mutex::new(WorkerHealth::Unreachable),
    }))
}

fn remember_unreachable(
    context: &Context,
    session_id: &str,
    pane_id: &str,
    worker_identity: ProcessIdentity,
) {
    if let Some(handle) = unreachable_worker_handle(context, session_id, pane_id, worker_identity) {
        context
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(pane_id.to_string(), handle);
    }
}

fn call_worker(
    context: &Context,
    session_id: &str,
    operation: &str,
    payload: Value,
    timeout: Duration,
) -> Result<Value, ProtocolErrorBody> {
    let pane_id = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .find_pane_for_session(session_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("terminal_lost", "session has no Pane"))?
        .id;
    call_pane_worker(context, &pane_id.to_string(), operation, payload, timeout)
}

fn call_pane_worker(
    context: &Context,
    pane_id: &str,
    operation: &str,
    payload: Value,
    timeout: Duration,
) -> Result<Value, ProtocolErrorBody> {
    let handle = context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(pane_id)
        .cloned()
        .ok_or_else(|| error("terminal_lost", "Pane has no live worker"))?;
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

fn agent_command_state(value: &Value) -> Result<AgentCommandState, ProtocolErrorBody> {
    match value.as_str() {
        Some("starting") => Ok(AgentCommandState::Starting),
        Some("running") => Ok(AgentCommandState::Running),
        Some("idle") => Ok(AgentCommandState::Idle),
        Some("done") => Ok(AgentCommandState::Done),
        Some("failed") => Ok(AgentCommandState::Failed),
        Some("lost") => Ok(AgentCommandState::Lost),
        _ => Err(error(
            "worker_contract",
            "agent command status has an invalid state",
        )),
    }
}

fn agent_command_record(
    pane: &PaneRecord,
    status: &Value,
) -> Result<AgentCommandRecord, ProtocolErrorBody> {
    if status.get("session_id").and_then(Value::as_str) != Some(pane.session_id.as_str()) {
        return Err(error(
            "worker_contract",
            "agent command reported the wrong SessionId",
        ));
    }
    let pane_id = pane.id.to_string();
    if status.get("pane_id").and_then(Value::as_str) != Some(pane_id.as_str()) {
        return Err(error(
            "worker_contract",
            "agent command reported the wrong PaneId",
        ));
    }
    let id = status
        .get("command_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("worker_contract", "agent command has no command_id"))?
        .parse()
        .map_err(|failure: uuid::Error| error("worker_contract", failure.to_string()))?;
    let owner = status
        .get("owner")
        .and_then(Value::as_str)
        .ok_or_else(|| error("worker_contract", "agent command has no owner"))?
        .to_string();
    let worker_identity = serde_json::from_value(
        status
            .get("worker_identity")
            .cloned()
            .ok_or_else(|| error("worker_contract", "agent command has no worker identity"))?,
    )
    .map_err(|failure| error("worker_contract", failure.to_string()))?;
    let process_identity = status
        .get("process_identity")
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|failure| error("worker_contract", failure.to_string()))?;
    let output_mode = match status.get("output_mode").and_then(Value::as_str) {
        Some("pty_merged") => AgentOutputMode::PtyMerged,
        Some("pipe_split") => AgentOutputMode::PipeSplit,
        _ => {
            return Err(error(
                "worker_contract",
                "agent command has an invalid output mode",
            ));
        }
    };
    let sandbox_risk = status
        .get("sandbox")
        .and_then(|sandbox| sandbox.get("risk"))
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value::<SandboxRiskRecord>)
        .transpose()
        .map_err(|failure| error("worker_contract", failure.to_string()))?;
    Ok(AgentCommandRecord {
        id,
        session_id: pane.session_id.clone(),
        pane_id: pane.id.clone(),
        owner,
        principal_id: qwe_core::PrincipalId::local_root(),
        creator_capability_id: None,
        worker_identity,
        process_identity,
        state: agent_command_state(
            status
                .get("state")
                .ok_or_else(|| error("worker_contract", "agent command has no state"))?,
        )?,
        output_mode,
        created_at_ms: status
            .get("created_at_ms")
            .and_then(Value::as_i64)
            .ok_or_else(|| error("worker_contract", "agent command has no created_at_ms"))?,
        updated_at_ms: status
            .get("updated_at_ms")
            .and_then(Value::as_i64)
            .ok_or_else(|| error("worker_contract", "agent command has no updated_at_ms"))?,
        exit_code: status
            .get("exit_code")
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok()),
        exit_signal: status
            .get("exit_signal")
            .and_then(Value::as_str)
            .map(str::to_string),
        sandbox_risk,
    })
}

fn persist_agent_status(
    context: &Context,
    pane: &PaneRecord,
    status: &Value,
) -> Result<AgentCommandRecord, ProtocolErrorBody> {
    let mut record = agent_command_record(pane, status)?;
    let store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = store
        .find_agent_command(&record.id)
        .map_err(storage_error)?
    {
        record.principal_id = existing.principal_id;
        record.creator_capability_id = existing.creator_capability_id;
        record.sandbox_risk = existing.sandbox_risk.or(record.sandbox_risk);
        drop(store);
        context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .update_agent_command_status(&record)
            .map_err(storage_error)?;
    } else {
        drop(store);
        context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .upsert_agent_command(&record)
            .map_err(storage_error)?;
    }
    Ok(record)
}

fn persist_agent_exec_status(
    context: &Context,
    pane: &PaneRecord,
    status: &Value,
    grant: &security::SecurityGrant,
) -> Result<AgentCommandRecord, ProtocolErrorBody> {
    let mut record = agent_command_record(pane, status)?;
    record.principal_id = grant.principal.id.clone();
    record.creator_capability_id = Some(grant.capability.id.clone());
    context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .upsert_agent_command(&record)
        .map_err(storage_error)?;
    Ok(record)
}

fn find_agent_command(
    context: &Context,
    command_id: &AgentCommandId,
) -> Result<AgentCommandRecord, ProtocolErrorBody> {
    context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .find_agent_command(command_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "agent command was not found"))
}

fn mark_agent_command_lost(
    context: &Context,
    command: &AgentCommandRecord,
) -> Result<AgentCommandRecord, ProtocolErrorBody> {
    let mut lost = command.clone();
    lost.state = AgentCommandState::Lost;
    lost.updated_at_ms = now_ms();
    context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .upsert_agent_command(&lost)
        .map_err(storage_error)?;
    Ok(lost)
}

fn sandbox_json_for_command(
    context: &Context,
    command: &AgentCommandRecord,
) -> Result<Option<Value>, ProtocolErrorBody> {
    let Some(capability_id) = command.creator_capability_id.as_ref() else {
        return Ok(None);
    };
    let store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let capability = store
        .find_capability(capability_id)
        .map_err(storage_error)?;
    let Some(capability) = capability else {
        return Ok(None);
    };
    let Some(profile) = capability.sandbox_profile else {
        return Ok(None);
    };
    Ok(Some(json!({
        "enforced": true,
        "profile": profile,
        "audit": capability.sandbox_audit,
        "risk": command.sandbox_risk,
        "wrapper_pid": command.process_identity.as_ref().map(|identity| identity.pid),
        "child_pid": null
    })))
}

fn add_sandbox_audit(
    context: &Context,
    command: &AgentCommandRecord,
    value: &mut Value,
) -> Result<(), ProtocolErrorBody> {
    let Some(capability_id) = command.creator_capability_id.as_ref() else {
        return Ok(());
    };
    let audit = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .find_capability(capability_id)
        .map_err(storage_error)?
        .and_then(|capability| capability.sandbox_audit);
    if let Some(audit) = audit
        && let Some(sandbox) = value.get_mut("sandbox").and_then(Value::as_object_mut)
    {
        sandbox.insert("audit".into(), json!(audit));
    }
    Ok(())
}

fn agent_record_json(command: &AgentCommandRecord, sandbox: Option<&Value>) -> Value {
    json!({
        "command_id": command.id,
        "session_id": command.session_id,
        "pane_id": command.pane_id,
        "owner": command.owner,
        "worker_identity": command.worker_identity,
        "process_identity": command.process_identity,
        "state": command.state.as_str(),
        "output_mode": command.output_mode.as_str(),
        "created_at_ms": command.created_at_ms,
        "updated_at_ms": command.updated_at_ms,
        "exit_code": command.exit_code,
        "exit_signal": command.exit_signal,
        "sandbox_risk": command.sandbox_risk,
        "sandbox": sandbox
    })
}

fn agent_command_operation(
    context: &Context,
    operation: &str,
    payload: &Value,
    auth_context: Option<&qwe_protocol::AuthContext>,
) -> Result<Value, ProtocolErrorBody> {
    let command_id: AgentCommandId = payload
        .get("command_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing command_id"))?
        .parse()
        .map_err(|failure: uuid::Error| error("invalid_request", failure.to_string()))?;
    let command = find_agent_command(context, &command_id)?;
    let full_diagnostics = {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let grant = if matches!(
            operation,
            "AGENT_COMMAND_TERMINATE" | "AGENT_COMMAND_SIGNAL"
        ) {
            security::require_command_terminate(&store, auth_context, &command, now_ms())?
        } else {
            security::require_command_read(&store, auth_context, &command, now_ms())?
        };
        grant.principal.high_trust
    };
    let pane_key = command.pane_id.to_string();
    let timeout = if operation == "AGENT_COMMAND_WAIT_IDLE" {
        WORKER_AGENT_WAIT_TIMEOUT
    } else {
        WORKER_REQUEST_TIMEOUT
    };
    let mut worker_payload = payload.clone();
    if operation == "AGENT_COMMAND_SIGNAL" {
        let signal = payload
            .get("signal")
            .and_then(Value::as_str)
            .ok_or_else(|| error("invalid_request", "missing signal"))?
            .parse::<AgentSignal>()
            .map_err(|failure| error("invalid_request", failure))?;
        worker_payload["signal"] = json!(signal.as_str());
    }
    if operation == "AGENT_COMMAND_WAIT_IDLE" {
        let timeout_ms = payload
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(AGENT_MAX_WAIT_MS)
            .min(AGENT_MAX_WAIT_MS);
        worker_payload["timeout_ms"] = json!(timeout_ms);
    }
    let result = call_pane_worker(context, &pane_key, operation, worker_payload, timeout);
    let mut value = match result {
        Ok(value) => value,
        Err(failure) if matches!(failure.code.as_str(), "not_found" | "terminal_lost") => {
            let lost = mark_agent_command_lost(context, &command)?;
            let sandbox = sandbox_json_for_command(context, &lost)?;
            return Ok(match operation {
                "AGENT_COMMAND_STATUS" | "AGENT_COMMAND_TERMINATE" | "AGENT_COMMAND_SIGNAL" => {
                    agent_record_json(&lost, sandbox.as_ref())
                }
                "AGENT_COMMAND_WAIT_IDLE" => json!({
                    "command_id": lost.id,
                    "state": "lost",
                    "wait_state": "lost",
                    "exit_code": lost.exit_code,
                    "exit_signal": lost.exit_signal,
                    "timed_out": false,
                    "sandbox": sandbox
                }),
                "AGENT_COMMAND_EVENTS" => json!({
                    "command_id": lost.id,
                    "state": "lost",
                    "cursor": payload.get("cursor").and_then(Value::as_u64).unwrap_or(0),
                    "next_cursor": payload.get("cursor").and_then(Value::as_u64).unwrap_or(0),
                    "replay_from": null,
                    "replay_gap": {"reason": "command_state_lost"},
                    "events": [],
                    "sandbox": sandbox
                }),
                _ => agent_record_json(&lost, sandbox.as_ref()),
            });
        }
        Err(failure) => return Err(failure),
    };
    add_sandbox_audit(context, &command, &mut value)?;
    if let Some(error_details) = value.get_mut("error_details") {
        redact_sandbox_value(error_details, full_diagnostics);
    }
    let pane = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .find_pane(&command.pane_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "Pane not found"))?;
    if matches!(
        operation,
        "AGENT_COMMAND_STATUS" | "AGENT_COMMAND_TERMINATE" | "AGENT_COMMAND_SIGNAL"
    ) {
        persist_agent_status(context, &pane, &value)?;
    } else if let Ok(status) = call_pane_worker(
        context,
        &pane_key,
        "AGENT_COMMAND_STATUS",
        json!({"command_id": command.id}),
        WORKER_REQUEST_TIMEOUT,
    ) {
        persist_agent_status(context, &pane, &status)?;
    }
    Ok(value)
}

fn connect_existing_worker(
    context: &Context,
    session_id: &str,
    pane_id: &str,
    socket: PathBuf,
) -> Result<(Arc<WorkerClient>, Value), String> {
    let client = WorkerClient::connect(&socket, session_id, pane_id, context.worker_events.clone())
        .map_err(|failure| failure.to_string())?;
    let status = client
        .call("WORKER_HELLO", json!({}), WORKER_CALL_TIMEOUT)
        .map_err(|failure| format!("{}: {}", failure.code, failure.message))?;
    Ok((client, status))
}

fn connect_health_client(
    context: &Context,
    session_id: &str,
    pane_id: &str,
    socket: &Path,
) -> Result<Arc<WorkerClient>, String> {
    let client = WorkerClient::connect(socket, session_id, pane_id, context.worker_events.clone())
        .map_err(|failure| failure.to_string())?;
    client
        .call("PING", json!({}), WORKER_CALL_TIMEOUT)
        .map_err(|failure| format!("{}: {}", failure.code, failure.message))?;
    Ok(client)
}

fn spawn_worker(
    context: &Context,
    spec: WorkerSpawnSpec<'_>,
) -> Result<(WorkerHandle, Value), ProtocolErrorBody> {
    let WorkerSpawnSpec {
        session_id,
        pane_id,
        program,
        arguments,
        resource_policy,
        credential,
        sandbox_profile,
    } = spec;
    let pane_key = pane_id.to_string();
    let socket = worker_socket(&context.runtime_dir, &pane_key);
    let attach_socket = worker_attach_socket(&context.runtime_dir, &pane_key);
    let _ = fs::remove_file(&socket);
    let _ = fs::remove_file(&attach_socket);
    let executable =
        worker_executable().map_err(|failure| error("internal", failure.to_string()))?;
    let mut command = if context.resource_support.hard_scope_available() {
        let mut command = Command::new("/usr/bin/systemd-run");
        command.args([
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            "--unit",
            &qwe_runtime::pane_scope_unit(&pane_key),
            "--property=MemoryAccounting=yes",
            "--property=TasksAccounting=yes",
            "--property=CPUAccounting=yes",
            "--property=Delegate=yes",
        ]);
        command.arg("--").arg(executable);
        command
    } else {
        Command::new(executable)
    };
    command
        .arg("--worker-socket")
        .arg(&socket)
        .arg("--worker-session-id")
        .arg(session_id)
        .arg("--worker-pane-id")
        .arg(pane_id.to_string())
        .arg("--worker-program")
        .arg(program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    for argument in arguments {
        command.arg("--worker-arg").arg(argument);
    }
    if context.resource_support.hard_scope_available() {
        command.arg("--worker-existing-scope");
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
    if let Some(profile) = sandbox_profile {
        command.arg("--worker-sandbox-profile").arg(
            serde_json::to_string(profile)
                .map_err(|failure| error("internal", failure.to_string()))?,
        );
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
            let _ = qwe_runtime::stop_pane_scope(&pane_id.to_string());
            return Err(error(
                "worker_start_failed",
                format!("worker exited during startup with {status}"),
            ));
        }
        match connect_existing_worker(context, session_id, &pane_key, socket.clone()) {
            Ok((client, status)) => {
                let health_client =
                    match connect_health_client(context, session_id, &pane_key, &socket) {
                        Ok(client) => client,
                        Err(failure) => {
                            last_failure = failure;
                            thread::sleep(Duration::from_millis(20));
                            continue;
                        }
                    };
                return worker_handle(
                    session_id,
                    &pane_key,
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
    let _ = qwe_runtime::stop_pane_scope(&pane_id.to_string());
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

fn start_command_credential_broker(runtime_dir: &Path, secret: &[u8]) -> std::io::Result<PathBuf> {
    let credential_fd = credential_pipe(secret)?;
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random)
        .map_err(|failure| std::io::Error::other(format!("random source failed: {failure}")))?;
    let path = runtime_dir.join(format!(
        "cmd-cred-{}-{}.sock",
        std::process::id(),
        hex_encode(&random)
    ));
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let cleanup_path = path.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = send_descriptor(&stream, credential_fd.as_raw_fd());
                    break;
                }
                Err(failure) if failure.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        let _ = fs::remove_file(cleanup_path);
    });
    Ok(path)
}

fn send_descriptor(
    stream: &UnixStream,
    descriptor: std::os::unix::io::RawFd,
) -> std::io::Result<()> {
    let byte = *b"C";
    let mut iovec = libc::iovec {
        iov_base: byte.as_ptr().cast_mut().cast(),
        iov_len: byte.len(),
    };
    let control_len =
        unsafe { libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as u32) } as usize;
    let mut control = vec![0_u8; control_len];
    let mut message = unsafe { std::mem::zeroed::<libc::msghdr>() };
    message.msg_iov = &mut iovec;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    let header = unsafe { libc::CMSG_FIRSTHDR(&message) };
    if header.is_null() {
        return Err(std::io::Error::other(
            "failed to build descriptor control message",
        ));
    }
    unsafe {
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as u32) as _;
        std::ptr::write(
            libc::CMSG_DATA(header).cast::<libc::c_int>(),
            descriptor as libc::c_int,
        );
        message.msg_controllen = (*header).cmsg_len;
    }
    let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &message, 0) };
    if sent < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
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
        "pane_id": task.pane_id,
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

fn resource_profile_name(policy: &ResourcePolicy) -> &'static str {
    ResourceProfile::classify(policy).map_or_else(
        || {
            if policy.is_empty() {
                "unlimited"
            } else {
                "custom"
            }
        },
        ResourceProfile::as_str,
    )
}

fn annotate_resource_status(status: &mut Value, requested: &ResourcePolicy) {
    if status.get("effective_resource_policy").is_none()
        || status.get("effective_resource_policy") == Some(&Value::Null)
    {
        status["effective_resource_policy"] = Value::Null;
    }
    status["resource_profile"] = json!(resource_profile_name(requested));
    status["requested_resource_policy"] = json!(requested);
    status["resource_policy"] = json!(requested);
}

fn sandbox_record(
    pane_id: &PaneId,
    grant: &security::SecurityGrant,
    created_at_ms: i64,
) -> qwe_core::PaneSandboxRecord {
    qwe_core::PaneSandboxRecord {
        pane_id: pane_id.clone(),
        profile: grant
            .capability
            .sandbox_profile
            .clone()
            .expect("validated sandbox capability has a profile"),
        audit: grant
            .capability
            .sandbox_audit
            .clone()
            .expect("validated sandbox capability has an audit"),
        capability_id: grant.capability.id.clone(),
        created_at_ms,
    }
}

fn validate_sandbox_worker_status(
    status: &Value,
    pane_id: &PaneId,
    grant: Option<&security::SecurityGrant>,
) -> Result<(), ProtocolErrorBody> {
    let enforced = status
        .get("sandbox")
        .and_then(|sandbox| sandbox.get("enforced"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match grant {
        Some(grant) => {
            let expected = grant
                .capability
                .sandbox_profile
                .as_ref()
                .ok_or_else(|| error("capability_invalid", "sandbox profile is missing"))?;
            if !enforced {
                return Err(error(
                    "worker_contract",
                    format!("Pane {pane_id} did not enforce its sandbox profile"),
                ));
            }
            if status.pointer("/sandbox/profile") != Some(&json!(expected)) {
                return Err(error(
                    "worker_contract",
                    format!("Pane {pane_id} reported a different sandbox profile"),
                ));
            }
        }
        None if enforced => {
            return Err(error(
                "worker_contract",
                format!("Pane {pane_id} unexpectedly enforced a sandbox profile"),
            ));
        }
        None => {}
    }
    Ok(())
}

fn annotate_sandbox_status(status: &mut Value, grant: Option<&security::SecurityGrant>) {
    let Some(grant) = grant else {
        return;
    };
    if !status.get("sandbox").is_some_and(Value::is_object) {
        status["sandbox"] = json!({"enforced": true});
    }
    status["sandbox"]["audit"] = json!(grant.capability.sandbox_audit);
    status["sandbox"]["capability_id"] = json!(grant.capability.id);
}

fn annotate_persisted_sandbox(status: &mut Value, sandbox: Option<&qwe_core::PaneSandboxRecord>) {
    let Some(sandbox) = sandbox else {
        return;
    };
    if !status.get("sandbox").is_some_and(Value::is_object) {
        status["sandbox"] = json!({"enforced": true, "backend": "bubblewrap"});
    }
    status["sandbox"]["enforced"] = json!(true);
    status["sandbox"]["profile"] = json!(sandbox.profile);
    status["sandbox"]["audit"] = json!(sandbox.audit);
    status["sandbox"]["capability_id"] = json!(sandbox.capability_id);
}

fn reject_sandboxed_input(context: &Context, pane_id: &PaneId) -> Result<(), ProtocolErrorBody> {
    let sandbox = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .find_pane_sandbox(pane_id)
        .map_err(storage_error)?;
    if sandbox.is_some() {
        return Err(error(
            "sandbox_input_audit_unavailable",
            "sandboxed Pane input is disabled until bounded input audit is implemented",
        ));
    }
    Ok(())
}

fn create_session(
    context: &Context,
    payload: Value,
    auth_context: Option<&qwe_protocol::AuthContext>,
) -> Result<Value, ProtocolErrorBody> {
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
    if payload.get("sandbox_profile").is_some() {
        return Err(error(
            "sandbox_profile_not_client_selectable",
            "sandbox profiles are selected only by capability issuance",
        ));
    }
    let sandboxed = match payload.get("sandboxed") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(|| {
            error(
                "invalid_request",
                "sandboxed must be a boolean when provided",
            )
        })?,
    };
    let sandbox_grant = if sandboxed {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Some(security::require_sandboxed_pane(
            &store,
            auth_context,
            &session_id,
            now_ms(),
        )?)
    } else {
        if auth_context.is_some() {
            let store = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let grant = security::require_live_token(&store, auth_context, now_ms())?;
            if grant.capability.operation == CapabilityOperation::SandboxedPane {
                return Err(error(
                    "sandbox_capability_requires_sandboxed_pane",
                    "sandboxed_pane capability cannot create an unsandboxed Session",
                ));
            }
        }
        None
    };
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
    let (_, resource_policy) = resolve_resource_policy(&payload, context.resource_support)?;
    let credential_name = match payload.get("credential") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if value.is_empty() => None,
        Some(Value::String(value)) => Some(value.as_str()),
        Some(_) => {
            return Err(error("invalid_request", "credential must be a string"));
        }
    };
    if sandboxed && credential_name.is_some() {
        return Err(error(
            "invalid_request",
            "sandboxed Panes do not accept long-lived credentials",
        ));
    }
    let credential = credential_name
        .map(|name| {
            context
                .credentials
                .read(name)
                .map(|secret| (name.to_string(), secret))
                .map_err(|failure| error("credential_unavailable", failure.to_string()))
        })
        .transpose()?;
    let pane_id = PaneId::new();
    let (handle, mut status) = spawn_worker(
        context,
        WorkerSpawnSpec {
            session_id: &session_id,
            pane_id: &pane_id,
            program,
            arguments: &arguments,
            resource_policy: &resource_policy,
            credential: credential
                .as_ref()
                .map(|(name, secret)| (name.as_str(), secret)),
            sandbox_profile: sandbox_grant
                .as_ref()
                .and_then(|grant| grant.capability.sandbox_profile.as_ref()),
        },
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
    let persisted = (|| {
        validate_sandbox_worker_status(&status, &pane_id, sandbox_grant.as_ref())?;
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
            resource_scope_id: ResourceScopeId::new(),
            resource_policy: resource_policy.clone(),
            kind: PaneKind::Normal,
            layout_order: 0,
            layout_weight: 1,
            z_index: 0,
            floating_rect: None,
            updated_at_ms: now,
        };
        let task = TaskRecord {
            id: task_id.clone(),
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
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
        let pane_sandbox = sandbox_grant
            .as_ref()
            .map(|grant| sandbox_record(&pane_id, grant, now));
        context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert_created_session_with_sandbox(&session, &pane, &task, pane_sandbox.as_ref())
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
        cleanup_worker_scope(&handle);
        let _ = fs::remove_file(&handle.socket);
        let _ = fs::remove_file(&handle.attach_socket);
        return Err(failure);
    }
    context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(pane_id.to_string(), Arc::new(handle));
    status["task_id"] = json!(task_id);
    status["pane_id"] = json!(pane_id);
    annotate_resource_status(&mut status, &resource_policy);
    annotate_sandbox_status(&mut status, sandbox_grant.as_ref());
    if let Some(response) = status.as_object_mut() {
        response.remove("total_written");
        response.remove("truncated_from");
    }
    Ok(status)
}

fn create_pane(
    context: &Context,
    payload: Value,
    auth_context: Option<&qwe_protocol::AuthContext>,
) -> Result<Value, ProtocolErrorBody> {
    let _creation = context
        .creation_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let session_id = required_string(&payload, "session_id")?.to_string();
    {
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let session = store
            .find_session(&session_id)
            .map_err(storage_error)?
            .ok_or_else(|| error("not_found", "Session not found"))?;
        if matches!(session.state.as_str(), "closing" | "closed" | "lost") {
            return Err(error(
                "invalid_state",
                format!("Session is '{}'", session.state),
            ));
        }
        authorize_session_control(
            &mut store,
            &session_id,
            &payload,
            AttachmentOperation::PaneCreate,
            now_ms(),
        )?;
    }
    if payload.get("sandbox_profile").is_some() {
        return Err(error(
            "sandbox_profile_not_client_selectable",
            "sandbox profiles are selected only by capability issuance",
        ));
    }
    let sandboxed = match payload.get("sandboxed") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(|| {
            error(
                "invalid_request",
                "sandboxed must be a boolean when provided",
            )
        })?,
    };
    let sandbox_grant = if sandboxed {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Some(security::require_sandboxed_pane(
            &store,
            auth_context,
            &session_id,
            now_ms(),
        )?)
    } else {
        if auth_context.is_some() {
            let store = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let grant = security::require_live_token(&store, auth_context, now_ms())?;
            if grant.capability.operation == CapabilityOperation::SandboxedPane {
                return Err(error(
                    "sandbox_capability_requires_sandboxed_pane",
                    "sandboxed_pane capability cannot create an unsandboxed Pane",
                ));
            }
        }
        None
    };
    let after_pane_id = payload
        .get("after_pane_id")
        .and_then(Value::as_str)
        .map(|value| {
            PaneId::from_str(value).map_err(|failure| error("invalid_request", failure.to_string()))
        })
        .transpose()?;
    let pane_kind = payload
        .get("pane_kind")
        .and_then(Value::as_str)
        .unwrap_or("normal")
        .parse::<PaneKind>()
        .map_err(|failure| error("invalid_request", failure))?;
    if pane_kind == PaneKind::Floating && after_pane_id.is_some() {
        return Err(error(
            "invalid_request",
            "after_pane_id is only valid for tiled Panes",
        ));
    }
    let (layout_order, z_index) = {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(after_pane_id) = after_pane_id.as_ref() {
            let after = store
                .find_pane(after_pane_id)
                .map_err(storage_error)?
                .filter(|pane| pane.session_id == session_id && pane.kind == PaneKind::Normal)
                .ok_or_else(|| {
                    error(
                        "invalid_request",
                        "after_pane_id must be a tiled Pane in the Session",
                    )
                })?;
            (
                after
                    .layout_order
                    .checked_add(1)
                    .ok_or_else(|| error("invalid_request", "Pane layout order overflowed"))?,
                0,
            )
        } else if pane_kind == PaneKind::Floating {
            (
                store
                    .next_pane_layout_order(&session_id)
                    .map_err(storage_error)?,
                store
                    .next_pane_z_index(&session_id)
                    .map_err(storage_error)?,
            )
        } else {
            let next = store
                .list_tiled_panes_for_session(&session_id)
                .map_err(storage_error)?
                .into_iter()
                .map(|pane| pane.layout_order)
                .max()
                .map_or(0, |order| order.saturating_add(1));
            (next, 0)
        }
    };
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
    let (_, resource_policy) = resolve_resource_policy(&payload, context.resource_support)?;
    let credential_name = match payload.get("credential") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if value.is_empty() => None,
        Some(Value::String(value)) => Some(value.as_str()),
        Some(_) => return Err(error("invalid_request", "credential must be a string")),
    };
    if sandboxed && credential_name.is_some() {
        return Err(error(
            "invalid_request",
            "sandboxed Panes do not accept long-lived credentials",
        ));
    }
    let credential = credential_name
        .map(|name| {
            context
                .credentials
                .read(name)
                .map(|secret| (name.to_string(), secret))
                .map_err(|failure| error("credential_unavailable", failure.to_string()))
        })
        .transpose()?;
    let pane_id = PaneId::new();
    let task_id = TaskId::new();
    let (handle, mut status) = spawn_worker(
        context,
        WorkerSpawnSpec {
            session_id: &session_id,
            pane_id: &pane_id,
            program,
            arguments: &arguments,
            resource_policy: &resource_policy,
            credential: credential
                .as_ref()
                .map(|(name, secret)| (name.as_str(), secret)),
            sandbox_profile: sandbox_grant
                .as_ref()
                .and_then(|grant| grant.capability.sandbox_profile.as_ref()),
        },
    )?;
    let now = now_ms();
    let running = status.get("running").and_then(Value::as_bool) == Some(true);
    let pane = PaneRecord {
        id: pane_id.clone(),
        session_id: session_id.clone(),
        resource_scope_id: ResourceScopeId::new(),
        resource_policy: resource_policy.clone(),
        kind: pane_kind,
        layout_order,
        layout_weight: 1,
        z_index,
        floating_rect: None,
        updated_at_ms: now,
    };
    let worker = PaneWorkerRecord {
        pane_id: pane_id.clone(),
        identity: handle.worker_identity.clone(),
        state: if running { "active" } else { "exited" }.into(),
        updated_at_ms: now,
    };
    let persisted = (|| {
        validate_sandbox_worker_status(&status, &pane_id, sandbox_grant.as_ref())?;
        let identity = parse_process_identity(&status)?;
        let task = TaskRecord {
            id: task_id.clone(),
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            identity,
            state: if running {
                TaskState::Running
            } else {
                TaskState::Exited
            },
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
        let pane_sandbox = sandbox_grant
            .as_ref()
            .map(|grant| sandbox_record(&pane_id, grant, now));
        context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert_created_pane_with_sandbox(&pane, &worker, &task, pane_sandbox.as_ref())
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
        cleanup_worker_scope(&handle);
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
        .insert(pane_id.to_string(), Arc::new(handle));
    status["session_id"] = json!(session_id);
    status["pane_id"] = json!(pane_id);
    status["task_id"] = json!(task_id);
    status["pane_kind"] = json!(pane.kind.as_str());
    status["layout_order"] = json!(pane.layout_order);
    status["layout_weight"] = json!(pane.layout_weight);
    status["z_index"] = json!(pane.z_index);
    status["floating_rect"] = json!(pane.floating_rect);
    annotate_resource_status(&mut status, &resource_policy);
    annotate_sandbox_status(&mut status, sandbox_grant.as_ref());
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
        .values()
        .find(|handle| handle.session_id == session_id)
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

fn apply_pane_status(
    context: &Context,
    pane_id: &PaneId,
    status: &Value,
) -> Result<(), ProtocolErrorBody> {
    let session_id = required_string(status, "session_id")?.to_string();
    let (_, worker_identity) = parse_worker_identity(status)?;
    let running = status.get("running").and_then(Value::as_bool) == Some(true);
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    store
        .upsert_pane_worker(&qwe_core::PaneWorkerRecord {
            pane_id: pane_id.clone(),
            identity: worker_identity.clone(),
            state: if running { "active" } else { "exited" }.into(),
            updated_at_ms: now_ms(),
        })
        .map_err(storage_error)?;
    store
        .upsert_session(&session_record(
            &session_id,
            status,
            &worker_identity,
            if running { "running" } else { "exited" },
            now_ms(),
        )?)
        .map_err(storage_error)?;
    if let Some(task) = store.find_task_for_pane(pane_id).map_err(storage_error)? {
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
    let (session_id, pane_id) = {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let task = store
            .find_task(&id)
            .map_err(storage_error)?
            .ok_or_else(|| error("not_found", "task not found"))?;
        (task.session_id, task.pane_id)
    };
    let status = match call_pane_worker(
        context,
        &pane_id.to_string(),
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
    apply_pane_status(context, &pane_id, &status)?;
    let task = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .find_task(&id)
        .map_err(storage_error)?
        .ok_or_else(|| error("persistence_failure", "task disappeared after update"))?;
    Ok(json!({"task": task_json(&task), "terminal": status}))
}

fn session_layout(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    let session_id = required_string(payload, "session_id")?;
    let rows = u16::try_from(required_u64(payload, "rows")?)
        .map_err(|_| error("invalid_request", "rows must fit in u16"))?;
    let columns = u16::try_from(required_u64(payload, "columns")?)
        .map_err(|_| error("invalid_request", "columns must fit in u16"))?;
    let store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    store
        .find_session(session_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "Session not found"))?;
    let panes = store
        .list_layout_panes_for_session(session_id)
        .map_err(storage_error)?;
    if panes.is_empty() {
        return Err(error("not_found", "Session has no Panes"));
    }
    let requested_focus = payload
        .get("focused_pane_id")
        .and_then(Value::as_str)
        .map(|value| {
            PaneId::from_str(value).map_err(|failure| error("invalid_request", failure.to_string()))
        })
        .transpose()?;
    let pane_ids = panes.iter().map(|pane| pane.id.clone()).collect::<Vec<_>>();
    if requested_focus
        .as_ref()
        .is_some_and(|focused| !pane_ids.contains(focused))
    {
        return Err(error(
            "invalid_request",
            "focused_pane_id does not belong to the Session",
        ));
    }
    let persisted_focus = store.session_focus(session_id).map_err(storage_error)?;
    if persisted_focus
        .as_ref()
        .is_some_and(|focused| !pane_ids.contains(focused))
    {
        return Err(error(
            "persistence_failure",
            "persisted focus does not belong to the Session",
        ));
    }
    let focused = requested_focus.as_ref().or(persisted_focus.as_ref());
    let specs = panes
        .iter()
        .map(|pane| PaneSpec {
            pane_id: pane.id.clone(),
            kind: pane.kind,
            layout_order: pane.layout_order,
            layout_weight: pane.layout_weight,
            z_index: pane.z_index,
            floating_rect: pane.floating_rect,
        })
        .collect::<Vec<_>>();
    let layouts = tile_pane_specs(&specs, rows, columns, focused)
        .map_err(|failure| error("invalid_request", failure.to_string()))?;
    let layouts = layouts
        .into_iter()
        .map(|layout| {
            let pane_id = layout.pane_id.to_string();
            let worker_state = store
                .find_pane_worker(&layout.pane_id)
                .map_err(storage_error)?
                .map_or_else(|| "absent".to_string(), |worker| worker.state);
            let mut value = serde_json::to_value(layout)
                .map_err(|failure| error("internal", failure.to_string()))?;
            value["worker_state"] = json!(worker_state);
            value["pane_id"] = json!(pane_id);
            Ok(value)
        })
        .collect::<Result<Vec<_>, ProtocolErrorBody>>()?;
    Ok(json!({
        "session_id": session_id,
        "rows": rows,
        "columns": columns,
        "panes": layouts
    }))
}

fn update_session_resources(
    context: &Context,
    payload: &Value,
) -> Result<Value, ProtocolErrorBody> {
    let session_id = required_string(payload, "session_id")?.to_string();
    if payload.get("resource_profile").is_none() && payload.get("resource_policy").is_none() {
        return Err(error(
            "invalid_request",
            "missing resource_profile or resource_policy",
        ));
    }
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
        let next = if payload.get("resource_profile").is_some() {
            resolve_resource_policy(payload, context.resource_support)?.1
        } else {
            merge_resource_policy(
                &pane.resource_policy,
                payload
                    .get("resource_policy")
                    .ok_or_else(|| error("invalid_request", "missing resource_policy"))?,
                context.resource_support,
            )?
        };
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
    let mut status = json!({
        "session_id": session_id,
        "resource_policy": next,
        "terminal": terminal
    });
    annotate_resource_status(&mut status, &next);
    if let Some(terminal) = status.get_mut("terminal") {
        annotate_resource_status(terminal, &next);
    }
    Ok(status)
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
    let mut status = json!({
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
        "pane_id": pane.id
    });
    annotate_resource_status(&mut status, &pane.resource_policy);
    Ok(status)
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
    if authority.state == AttachmentAuthorityState::Unattached
        && matches!(
            operation,
            AttachmentOperation::PaneCreate
                | AttachmentOperation::ResourcePolicy
                | AttachmentOperation::CloseTaskStop
        )
    {
        let owner = payload.get("owner").and_then(Value::as_str);
        let generation = required_u64(payload, "generation").ok();
        if let Some(lease) = store.get_lease(session_id).map_err(storage_error)? {
            if owner == Some(lease.owner.as_str())
                && generation == Some(lease.generation)
                && lease.released_at_ms.is_none()
                && lease.deadline_ms > now_ms
            {
                return Ok(());
            }
        }
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

fn authorize_pane_control(
    store: &mut Store,
    session_id: &str,
    pane_id: &PaneId,
    payload: &Value,
    operation: AttachmentOperation,
    now_ms: i64,
) -> Result<(), ProtocolErrorBody> {
    let mut authority = store
        .get_attachment_authority_for_pane(pane_id)
        .map_err(storage_error)?
        .unwrap_or_else(AttachmentAuthority::unattached);
    if authority.state != authority.state_at(now_ms) {
        authority.release();
        store
            .write_attachment_authority_for_pane(pane_id, &authority, now_ms)
            .map_err(storage_error)?;
    }
    if authority.state == AttachmentAuthorityState::Unattached
        && operation == AttachmentOperation::LayoutMutation
    {
        let owner = payload.get("owner").and_then(Value::as_str);
        let generation = required_u64(payload, "generation").ok();
        if let Some(lease) = store.get_lease(session_id).map_err(storage_error)? {
            if owner == Some(lease.owner.as_str())
                && generation == Some(lease.generation)
                && lease.released_at_ms.is_none()
                && lease.deadline_ms > now_ms
            {
                return Ok(());
            }
        }
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

fn pane_focus(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    let pane_id = required_pane_id(payload)?;
    let now = now_ms();
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let pane = store
        .find_pane(&pane_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "Pane not found"))?;
    authorize_pane_control(
        &mut store,
        &pane.session_id,
        &pane_id,
        payload,
        AttachmentOperation::LayoutMutation,
        now,
    )?;
    store
        .set_session_focus(&pane.session_id, &pane_id, now)
        .map_err(storage_error)?;
    Ok(json!({
        "session_id": pane.session_id,
        "pane_id": pane_id,
        "focused_at_ms": now
    }))
}

fn pane_reorder(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    let pane_values = payload
        .get("pane_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| error("invalid_request", "missing pane_ids"))?;
    if pane_values.is_empty() {
        return Err(error("invalid_request", "pane_ids must not be empty"));
    }
    if pane_values.len() > qwe_core::MAX_LAYOUT_PANES {
        return Err(error(
            "invalid_request",
            format!(
                "pane_ids supports at most {} Panes",
                qwe_core::MAX_LAYOUT_PANES
            ),
        ));
    }
    let pane_ids = pane_values
        .iter()
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| error("invalid_request", "pane_ids must contain strings"))?;
            PaneId::from_str(value).map_err(|failure| error("invalid_request", failure.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if pane_ids.iter().collect::<HashSet<_>>().len() != pane_ids.len() {
        return Err(error("invalid_request", "pane_ids contains duplicates"));
    }

    let now = now_ms();
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let first = store
        .find_pane(&pane_ids[0])
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "Pane not found"))?;
    let expected = store
        .list_tiled_panes_for_session(&first.session_id)
        .map_err(storage_error)?
        .into_iter()
        .map(|pane| pane.id)
        .collect::<HashSet<_>>();
    if pane_ids.iter().cloned().collect::<HashSet<_>>() != expected {
        return Err(error(
            "invalid_request",
            "pane_ids must include every Pane in the Session exactly once",
        ));
    }
    authorize_session_control(
        &mut store,
        &first.session_id,
        payload,
        AttachmentOperation::LayoutMutation,
        now,
    )?;
    store
        .reorder_panes(&first.session_id, &pane_ids, now)
        .map_err(storage_error)?;
    Ok(json!({
        "session_id": first.session_id,
        "pane_ids": pane_ids,
        "updated_at_ms": now
    }))
}

fn pane_floating_update(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    let pane_id = required_pane_id(payload)?;
    let rect = payload
        .get("rect")
        .cloned()
        .ok_or_else(|| error("invalid_request", "missing rect"))
        .and_then(|value| {
            serde_json::from_value::<qwe_core::LayoutRect>(value)
                .map_err(|failure| error("invalid_request", failure.to_string()))
        })?;
    rect.validate()
        .map_err(|failure| error("invalid_request", failure.to_string()))?;
    let now = now_ms();
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let pane = store
        .find_pane(&pane_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "Pane not found"))?;
    authorize_pane_control(
        &mut store,
        &pane.session_id,
        &pane_id,
        payload,
        AttachmentOperation::LayoutMutation,
        now,
    )?;
    let pane = store
        .update_pane_floating_rect(&pane_id, rect, now)
        .map_err(storage_error)?;
    Ok(json!({
        "session_id": pane.session_id,
        "pane_id": pane.id,
        "rect": pane.floating_rect,
        "updated_at_ms": pane.updated_at_ms
    }))
}

fn pane_floating_raise(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    let pane_id = required_pane_id(payload)?;
    let now = now_ms();
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let pane = store
        .find_pane(&pane_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "Pane not found"))?;
    authorize_pane_control(
        &mut store,
        &pane.session_id,
        &pane_id,
        payload,
        AttachmentOperation::LayoutMutation,
        now,
    )?;
    let pane = store
        .raise_floating_pane(&pane_id, now)
        .map_err(storage_error)?;
    Ok(json!({
        "session_id": pane.session_id,
        "pane_id": pane.id,
        "z_index": pane.z_index,
        "updated_at_ms": pane.updated_at_ms
    }))
}

fn required_pane_id(payload: &Value) -> Result<PaneId, ProtocolErrorBody> {
    PaneId::from_str(required_string(payload, "pane_id")?)
        .map_err(|failure| error("invalid_request", failure.to_string()))
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
    let pane_key = task.pane_id.to_string();
    let status = call_pane_worker(
        context,
        &pane_key,
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
        .get(&pane_key)
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
            let pane_key = task.pane_id.to_string();
            let status = match call_pane_worker(
                context,
                &pane_key,
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
                .get(&pane_key)
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
    let panes = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .list_panes_for_session(&session_id)
        .map_err(storage_error)?;
    let worker_pane_keys = context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    let authority_pane_id = panes
        .iter()
        .find(|pane| worker_pane_keys.contains(&pane.id.to_string()))
        .or_else(|| panes.first())
        .map(|pane| pane.id.clone());
    let mut pane_results = Vec::new();
    for pane in &panes {
        let pane_key = pane.id.to_string();
        if !worker_pane_keys.contains(&pane_key) {
            pane_results.push(json!({
                "session_id": session_id,
                "pane_id": pane.id,
                "state": "closed",
                "running": false
            }));
            continue;
        }
        let result = match call_pane_worker(
            context,
            &pane_key,
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
                    .find_task_for_pane(&pane.id)
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
                    "pane_id": pane.id,
                    "state": "closed",
                    "forced": true,
                    "worker_error": failure
                })
            }
            Err(failure) => return Err(failure),
        };
        pane_results.push(result);
        let handle = context
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&pane_key);
        if let Some(handle) = &handle {
            cleanup_worker_scope(handle);
            unsafe {
                libc::kill(
                    handle.worker_pid as libc::pid_t,
                    if forced { libc::SIGKILL } else { libc::SIGTERM },
                );
            }
            let _ = fs::remove_file(&handle.socket);
            let _ = fs::remove_file(&handle.attach_socket);
        }
    }
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = now_ms();
    store
        .close_session_pane_records(&session_id, now)
        .map_err(storage_error)?;
    if let Some(mut session) = store.find_session(&session_id).map_err(storage_error)? {
        session.state = "closed".into();
        session.updated_at_ms = now;
        store.upsert_session(&session).map_err(storage_error)?;
    }
    let mut authority = match authority_pane_id.as_ref() {
        Some(pane_id) => store
            .get_attachment_authority_for_pane(pane_id)
            .map_err(storage_error)?
            .unwrap_or_else(AttachmentAuthority::unattached),
        None => AttachmentAuthority::unattached(),
    };
    if let Some(lease) = store.get_lease(&session_id).map_err(storage_error)? {
        if lease.released_at_ms.is_none() {
            let released = store
                .release_lease(&session_id, &lease.owner, lease.generation, now_ms())
                .map_err(storage_error)?;
            authority.generation = released.generation;
        }
    }
    authority.release();
    if let Some(pane_id) = &authority_pane_id {
        store
            .write_attachment_authority_for_pane(pane_id, &authority, now_ms())
            .map_err(storage_error)?;
    }
    let mut response = pane_results
        .first()
        .cloned()
        .unwrap_or_else(|| json!({"session_id": session_id}));
    response["session_id"] = json!(session_id);
    response["state"] = json!("closed");
    response["running"] = json!(false);
    response["panes"] = json!(pane_results);
    Ok(response)
}

fn close_session(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    teardown_session(context, payload, "SESSION_CLOSE", false)
}

fn kill_session(context: &Context, payload: &Value) -> Result<Value, ProtocolErrorBody> {
    teardown_session(context, payload, "SESSION_KILL", true)
}

fn teardown_pane(
    context: &Context,
    payload: &Value,
    worker_operation: &str,
    forced: bool,
) -> Result<Value, ProtocolErrorBody> {
    let pane_id = required_pane_id(payload)?;
    let pane_key = pane_id.to_string();
    let pane = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .find_pane(&pane_id)
        .map_err(storage_error)?
        .ok_or_else(|| error("not_found", "Pane not found"))?;
    let mut worker_payload = payload.clone();
    worker_payload["session_id"] = json!(pane.session_id);
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
            &pane.session_id,
            payload,
            AttachmentOperation::CloseTaskStop,
            now_ms(),
        )?;
    }
    let mut result = match call_pane_worker(
        context,
        &pane_key,
        worker_operation,
        worker_payload,
        WORKER_MAINTENANCE_TIMEOUT,
    ) {
        Ok(result) => result,
        Err(failure) if forced => {
            if let Some(task) = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .find_task_for_pane(&pane_id)
                .map_err(storage_error)?
            {
                if qwe_runtime::process_matches(&task.identity) {
                    unsafe {
                        libc::kill(-(task.identity.pgid as libc::pid_t), libc::SIGKILL);
                    }
                }
            }
            json!({
                "session_id": pane.session_id,
                "pane_id": pane_id,
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
        .remove(&pane_key);
    if let Some(handle) = &handle {
        cleanup_worker_scope(handle);
        unsafe {
            libc::kill(
                handle.worker_pid as libc::pid_t,
                if forced { libc::SIGKILL } else { libc::SIGTERM },
            );
        }
        let _ = fs::remove_file(&handle.socket);
        let _ = fs::remove_file(&handle.attach_socket);
    }
    let now = now_ms();
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(task) = store.find_task_for_pane(&pane_id).map_err(storage_error)? {
        if task.state == TaskState::Running {
            store
                .update_task(&task.id, TaskState::Stopped, None, None, Some(now))
                .map_err(storage_error)?;
        }
    }
    if let Some(mut worker) = store.find_pane_worker(&pane_id).map_err(storage_error)? {
        worker.state = "closed".into();
        worker.updated_at_ms = now;
        store.upsert_pane_worker(&worker).map_err(storage_error)?;
    }
    store
        .update_resource_scope_state(
            &pane.resource_scope_id,
            qwe_core::ResourceScopeState::Closed,
            Some("complete scope stopped"),
            now,
        )
        .map_err(storage_error)?;
    let remaining_panes = store
        .list_layout_panes_for_session(&pane.session_id)
        .map_err(storage_error)?;
    if store
        .session_focus(&pane.session_id)
        .map_err(storage_error)?
        .is_none_or(|focused| focused == pane_id)
        && let Some(replacement) = remaining_panes
            .iter()
            .find(|candidate| candidate.id != pane_id)
    {
        store
            .set_session_focus(&pane.session_id, &replacement.id, now)
            .map_err(storage_error)?;
    }
    let any_active = remaining_panes
        .iter()
        .filter(|candidate| candidate.id != pane_id)
        .any(|candidate| {
            store
                .find_pane_worker(&candidate.id)
                .ok()
                .flatten()
                .is_some_and(|worker| worker.state == "active")
        });
    if !any_active {
        if let Some(mut session) = store
            .find_session(&pane.session_id)
            .map_err(storage_error)?
        {
            if session.state == "running" {
                session.state = "exited".into();
                session.updated_at_ms = now;
                store.upsert_session(&session).map_err(storage_error)?;
            }
        }
    }
    result["state"] = json!("closed");
    result["running"] = json!(false);
    result["pane_id"] = json!(pane_id);
    Ok(result)
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
        let panes = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .list_panes_for_session(&session.id)
            .map_err(storage_error)?;
        let mut pane_results = Vec::new();
        for pane in &panes {
            let pane_key = pane.id.to_string();
            let handle = context
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&pane_key);
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
                if let Some(task) = context
                    .store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .find_task_for_pane(&pane.id)
                    .map_err(storage_error)?
                {
                    if qwe_runtime::process_matches(&task.identity) {
                        unsafe {
                            libc::kill(-(task.identity.pgid as libc::pid_t), libc::SIGKILL);
                        }
                    }
                }
            }
            if let Some(handle) = &handle {
                cleanup_worker_scope(handle);
                unsafe {
                    libc::kill(handle.worker_pid as libc::pid_t, libc::SIGKILL);
                }
                let _ = fs::remove_file(&handle.socket);
                let _ = fs::remove_file(&handle.attach_socket);
            }
            pane_results.push((pane.id.clone(), worker_result));
        }
        let now = now_ms();
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let persistence = (|| {
            for (pane_id, worker_result) in &pane_results {
                if let Some(task) = store.find_task_for_pane(pane_id).map_err(storage_error)? {
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
                if let Some(mut worker) = store.find_pane_worker(pane_id).map_err(storage_error)? {
                    worker.state = "closed".into();
                    worker.updated_at_ms = now;
                    store.upsert_pane_worker(&worker).map_err(storage_error)?;
                }
            }
            for pane in &panes {
                store
                    .update_resource_scope_state(
                        &pane.resource_scope_id,
                        qwe_core::ResourceScopeState::Closed,
                        Some("server shutdown stopped complete scope"),
                        now,
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
    let _ = fs::remove_file(context.runtime_dir.join("security.bootstrap"));
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

fn mark_pane_lost(context: &Context, pane_id: &str) -> Result<(), StorageError> {
    let pane_id_value =
        PaneId::from_str(pane_id).map_err(|error| StorageError::Invariant(error.to_string()))?;
    let now = now_ms();
    let mut store = context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(pane) = store.find_pane(&pane_id_value)? else {
        return Ok(());
    };
    if let Some(mut worker) = store.find_pane_worker(&pane_id_value)? {
        worker.state = "lost".into();
        worker.updated_at_ms = now;
        store.upsert_pane_worker(&worker)?;
    }
    if let Some(task) = store.find_task_for_pane(&pane_id_value)? {
        if matches!(task.state, TaskState::Running | TaskState::Stopped) {
            store.update_task(&task.id, TaskState::Lost, None, None, Some(now))?;
        }
    }
    store.mark_agent_commands_lost_for_pane(&pane_id_value, now)?;
    let session_panes = store
        .list_panes_for_session(&pane.session_id)?
        .into_iter()
        .map(|candidate| candidate.id)
        .collect::<HashSet<_>>();
    let other_active = store.list_pane_workers()?.into_iter().any(|worker| {
        session_panes.contains(&worker.pane_id)
            && worker.pane_id != pane_id_value
            && worker.state == "active"
    });
    if !other_active {
        if let Some(mut session) = store.find_session(&pane.session_id)? {
            if !matches!(session.state.as_str(), "lost" | "closed") {
                session.state = "lost".into();
                session.updated_at_ms = now;
                store.upsert_session(&session)?;
            }
        }
    } else if let Some(mut session) = store.find_session(&pane.session_id)? {
        if session.state == "lost" {
            session.state = "running".into();
            session.updated_at_ms = now;
            store.upsert_session(&session)?;
        }
    }
    Ok(())
}

fn remove_worker_files(context: &Context, pane_id: &str) {
    let _ = fs::remove_file(worker_socket(&context.runtime_dir, pane_id));
    let _ = fs::remove_file(worker_attach_socket(&context.runtime_dir, pane_id));
}

fn quarantine_worker(context: &Context, pane_id: &str) {
    for path in [
        worker_socket(&context.runtime_dir, pane_id),
        worker_attach_socket(&context.runtime_dir, pane_id),
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
        "pane_id": incident.pane_id,
        "disposition": incident.disposition.as_str(),
        "health": incident.health.as_str(),
        "socket": incident.socket
    })
}

fn pane_requires_loss_update(worker_state: Option<&str>, task_state: Option<TaskState>) -> bool {
    match worker_state {
        Some("closed" | "exited") => !matches!(
            task_state,
            Some(TaskState::Exited | TaskState::Stopped | TaskState::Lost)
        ),
        Some("lost") => !matches!(task_state, Some(TaskState::Exited | TaskState::Lost)),
        _ => true,
    }
}

fn startup_reconcile(context: &Context) -> Result<Reconciliation, Box<dyn std::error::Error>> {
    let (sessions, panes, stored_workers, stored_tasks) = {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            store.list_sessions()?,
            store.list_panes()?,
            store.list_pane_workers()?,
            store.list_tasks()?,
        )
    };
    let sessions_by_id = sessions
        .iter()
        .map(|session| (session.id.clone(), session))
        .collect::<HashMap<_, _>>();
    let stored_workers = stored_workers
        .into_iter()
        .map(|worker| (worker.pane_id.clone(), worker))
        .collect::<HashMap<_, _>>();
    let stored_tasks = stored_tasks
        .into_iter()
        .map(|task| (task.pane_id.clone(), task))
        .collect::<HashMap<_, _>>();

    let mut expected_control_sockets = HashSet::new();
    let mut expected_attach_sockets = HashSet::new();
    for pane in &panes {
        let pane_key = pane.id.to_string();
        expected_control_sockets.insert(worker_socket(&context.runtime_dir, &pane_key));
        expected_attach_sockets.insert(worker_attach_socket(&context.runtime_dir, &pane_key));
        if let Some(session) = sessions_by_id.get(&pane.session_id) {
            expected_control_sockets
                .insert(legacy_worker_socket(&context.runtime_dir, &session.id));
            expected_attach_sockets.insert(legacy_worker_attach_socket(
                &context.runtime_dir,
                &session.id,
            ));
        }
    }

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

    let pane_by_session = panes.iter().fold(
        HashMap::<String, Vec<&PaneRecord>>::new(),
        |mut grouped, pane| {
            grouped
                .entry(pane.session_id.clone())
                .or_default()
                .push(pane);
            grouped
        },
    );
    let mut healthy_groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for (runtime_path, status) in &control_probes {
        let Some(status) = status else {
            continue;
        };
        let pane_id = status
            .get("pane_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                let session_id = status.get("session_id").and_then(Value::as_str)?;
                let candidates = pane_by_session.get(session_id)?;
                (candidates.len() == 1).then(|| candidates[0].id.to_string())
            });
        if let Some(pane_id) = pane_id {
            healthy_groups
                .entry(pane_id)
                .or_default()
                .push(runtime_path.clone());
        }
    }
    let duplicate_panes = healthy_groups
        .iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(pane_id, _)| pane_id.clone())
        .collect::<HashSet<_>>();
    let mut report = Reconciliation::default();
    for (pane_id, paths) in &healthy_groups {
        if paths.len() <= 1 {
            continue;
        }
        report.conflict_sessions += 1;
        for runtime_path in paths {
            report.incidents.push(RuntimeIncident {
                session_id: panes
                    .iter()
                    .find(|pane| pane.id.to_string() == *pane_id)
                    .map(|pane| pane.session_id.clone()),
                pane_id: Some(pane_id.clone()),
                disposition: ReconciliationDisposition::Conflict,
                health: WorkerHealth::Healthy,
                socket: runtime_path.clone(),
            });
            quarantine_runtime_family(runtime_path);
        }
    }

    for pane in panes {
        let pane_key = pane.id.to_string();
        if duplicate_panes.contains(&pane_key) {
            continue;
        }
        let Some(session) = sessions_by_id.get(&pane.session_id) else {
            continue;
        };
        if session.state == "closed" {
            let socket = worker_socket(&context.runtime_dir, &pane_key);
            if socket.exists() {
                report.conflict_sessions += 1;
                report.incidents.push(RuntimeIncident {
                    session_id: Some(session.id.clone()),
                    pane_id: Some(pane_key.clone()),
                    disposition: ReconciliationDisposition::Conflict,
                    health: if control_probes.get(&socket).is_some_and(Option::is_some) {
                        WorkerHealth::Healthy
                    } else {
                        WorkerHealth::Unreachable
                    },
                    socket: socket.clone(),
                });
                quarantine_worker(context, &pane_key);
            }
            continue;
        }
        let socket = worker_socket(&context.runtime_dir, &pane_key);
        let runtime_path = healthy_groups
            .get(&pane_key)
            .and_then(|paths| paths.first())
            .cloned()
            .or_else(|| socket.exists().then_some(socket.clone()));
        let Some(runtime_path) = runtime_path else {
            if let Some(worker) = stored_workers.get(&pane.id) {
                if qwe_runtime::process_matches(&worker.identity) {
                    remember_unreachable(context, &session.id, &pane_key, worker.identity.clone());
                } else if pane_requires_loss_update(
                    Some(&worker.state),
                    stored_tasks.get(&pane.id).map(|task| task.state),
                ) {
                    mark_pane_lost(context, &pane_key)?;
                }
            } else {
                mark_pane_lost(context, &pane_key)?;
            }
            continue;
        };
        match connect_existing_worker(context, &session.id, &pane_key, runtime_path.clone()) {
            Ok((client, status)) => {
                let health_client =
                    match connect_health_client(context, &session.id, &pane_key, &runtime_path) {
                        Ok(client) => client,
                        Err(failure) => {
                            eprintln!(
                                "worker health channel unavailable for Pane {}: {}",
                                pane_key, failure
                            );
                            if let Some(worker) = stored_workers.get(&pane.id) {
                                remember_unreachable(
                                    context,
                                    &session.id,
                                    &pane_key,
                                    worker.identity.clone(),
                                );
                            }
                            continue;
                        }
                    };
                match worker_handle(
                    &session.id,
                    &pane_key,
                    runtime_path.clone(),
                    &status,
                    Some(client.clone()),
                    Some(health_client),
                    WorkerHealth::Healthy,
                ) {
                    Ok(handle) => {
                        if stored_workers
                            .get(&pane.id)
                            .is_some_and(|worker| worker.identity != handle.worker_identity)
                        {
                            quarantine_worker(context, &pane_key);
                            mark_pane_lost(context, &pane_key)?;
                            report.conflict_sessions += 1;
                            report.incidents.push(RuntimeIncident {
                                session_id: Some(session.id.clone()),
                                pane_id: Some(pane_key.clone()),
                                disposition: ReconciliationDisposition::Conflict,
                                health: WorkerHealth::Healthy,
                                socket: runtime_path.clone(),
                            });
                            continue;
                        }
                        apply_pane_status(context, &pane.id, &status)
                            .map_err(|failure| format!("{}: {}", failure.code, failure.message))?;
                        context
                            .workers
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .insert(pane_key, Arc::new(handle));
                    }
                    Err(failure) => {
                        quarantine_worker(context, &pane_key);
                        mark_pane_lost(context, &pane_key)?;
                        report.conflict_sessions += 1;
                        report.incidents.push(RuntimeIncident {
                            session_id: Some(session.id.clone()),
                            pane_id: Some(pane_key.clone()),
                            disposition: ReconciliationDisposition::Conflict,
                            health: WorkerHealth::Healthy,
                            socket: runtime_path.clone(),
                        });
                        eprintln!(
                            "worker identity conflict for Pane {}: {}",
                            pane_key, failure.message
                        );
                    }
                }
            }
            Err(_) => {
                if let Some(worker) = stored_workers.get(&pane.id) {
                    if qwe_runtime::process_matches(&worker.identity) {
                        remember_unreachable(
                            context,
                            &session.id,
                            &pane_key,
                            worker.identity.clone(),
                        );
                    } else {
                        mark_pane_lost(context, &pane_key)?;
                    }
                } else {
                    mark_pane_lost(context, &pane_key)?;
                }
            }
        }
    }

    let mut orphan_ids = HashSet::new();
    for runtime_path in runtime_entries {
        if !runtime_path.exists()
            || expected_control_sockets.contains(&runtime_path)
            || expected_attach_sockets.contains(&runtime_path)
        {
            continue;
        }
        let probe = control_probes.get(&runtime_path).and_then(Option::as_ref);
        let session_id = probe
            .and_then(|status| status.get("session_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let pane_id = probe
            .and_then(|status| status.get("pane_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let health = if probe.is_some() {
            WorkerHealth::Healthy
        } else {
            WorkerHealth::Unreachable
        };
        let identity = pane_id
            .clone()
            .or_else(|| session_id.clone())
            .unwrap_or_else(|| runtime_path.display().to_string());
        orphan_ids.insert(identity);
        report.incidents.push(RuntimeIncident {
            session_id,
            pane_id,
            disposition: ReconciliationDisposition::Orphaned,
            health,
            socket: runtime_path,
        });
    }
    report.orphaned_sessions = orphan_ids.len();
    Ok(report)
}

fn reconnect_worker_handle(
    context: &Context,
    previous: &WorkerHandle,
) -> Option<Arc<WorkerHandle>> {
    if !qwe_runtime::process_matches(&previous.worker_identity) || !previous.socket.exists() {
        return None;
    }
    let (client, status) = connect_existing_worker(
        context,
        &previous.session_id,
        &previous.pane_id,
        previous.socket.clone(),
    )
    .ok()?;
    let health_client = connect_health_client(
        context,
        &previous.session_id,
        &previous.pane_id,
        &previous.socket,
    )
    .ok()?;
    let handle = worker_handle(
        &previous.session_id,
        &previous.pane_id,
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
    for (pane_id, handle) in handles {
        if !qwe_runtime::process_matches(&handle.worker_identity) {
            let _ = mark_pane_lost(context, &pane_id);
            context
                .workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&pane_id);
            remove_worker_files(context, &pane_id);
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
                    if let Some(reconnected) = reconnect_worker_handle(context, &handle) {
                        context
                            .workers
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .insert(pane_id.clone(), reconnected);
                    } else {
                        set_worker_health(&handle, WorkerHealth::Unreachable);
                    }
                } else {
                    set_worker_health(&handle, WorkerHealth::Healthy);
                }
                if let Ok(pane_id) = PaneId::from_str(&pane_id) {
                    let _ = apply_pane_status(context, &pane_id, &status);
                }
            }
            Err(_) => {
                set_worker_health(&handle, WorkerHealth::Unreachable);
                if let Some(reconnected) = reconnect_worker_handle(context, &handle) {
                    context
                        .workers
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .insert(pane_id.clone(), reconnected);
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
    let pane_workers = store.list_pane_workers().map_err(storage_error)?;
    let agent_commands = store
        .list_nonterminal_agent_commands()
        .map_err(storage_error)?;
    let resource_scopes = store.list_resource_scopes().map_err(storage_error)?;
    let principals = store.list_principals().map_err(storage_error)?;
    let workers = context
        .workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .map(|handle| worker_json(handle))
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
        "capabilities": capability_matrix(context.resource_support),
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
        "pane_workers": pane_workers,
        "resource_scopes": resource_scopes,
        "agent_commands": agent_commands,
        "principals": principals,
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

fn principal_json(principal: &qwe_core::PrincipalRecord) -> Value {
    json!({
        "principal_id": principal.id,
        "parent_id": principal.parent_id,
        "epoch": principal.epoch,
        "high_trust": principal.high_trust,
        "created_at_ms": principal.created_at_ms,
        "revoked_at_ms": principal.revoked_at_ms,
        "revocation": principal.revocation_severity.map(|severity| json!({
            "severity": severity.as_str(),
            "deadline_ms": principal.revocation_deadline_ms,
            "hold": principal.revocation_hold
        }))
    })
}

fn security_bootstrap(
    context: &Context,
    request: &RequestEnvelope,
) -> Result<Value, ProtocolErrorBody> {
    let grant = {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        security::require_management(&store, request.auth_context.as_ref(), now_ms())?
    };
    Ok(json!({
        "principal": principal_json(&grant.principal),
        "capability": {
            "capability_id": grant.capability.id,
            "operation": grant.capability.operation.as_str(),
            "scope": grant.capability.scope,
            "expires_at_ms": grant.capability.expires_at_ms
        }
    }))
}

fn security_principal_create(
    context: &Context,
    request: &RequestEnvelope,
) -> Result<Value, ProtocolErrorBody> {
    let parent_id = request
        .payload
        .get("parent_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing parent_id"))?
        .parse::<PrincipalId>()
        .map_err(|failure| error("invalid_request", failure.to_string()))?;
    let high_trust = request
        .payload
        .get("high_trust")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let principal = {
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let grant = security::require_management(&store, request.auth_context.as_ref(), now_ms())?;
        if high_trust && !grant.principal.high_trust {
            return Err(error(
                "insufficient_trust",
                "only a high-trust Principal may create a high-trust descendant",
            ));
        }
        if store
            .find_principal(&parent_id)
            .map_err(storage_error)?
            .is_some_and(|principal| principal.revoked_at_ms.is_some())
        {
            return Err(error(
                "principal_revoked",
                "cannot create a descendant below a revoked Principal",
            ));
        }
        let principal_id = PrincipalId::new();
        store
            .insert_principal(&principal_id, &parent_id, high_trust, now_ms())
            .map_err(storage_error)?;
        store
            .find_principal(&principal_id)
            .map_err(storage_error)?
            .ok_or_else(|| error("persistence_failure", "created Principal is missing"))?
    };
    Ok(principal_json(&principal))
}

fn probe_sandbox_worker(
    context: &Context,
    pane_id: &PaneId,
    profile: &SandboxProfile,
) -> Result<SandboxAuditRecord, ProtocolErrorBody> {
    let result = call_pane_worker(
        context,
        &pane_id.to_string(),
        "SANDBOX_PROBE",
        json!({"sandbox_profile": profile}),
        WORKER_REQUEST_TIMEOUT,
    )
    .map_err(map_sandbox_probe_failure)?;
    sandbox_audit_from_probe(profile, &result)
}

fn probe_sandbox_profile(
    context: &Context,
    profile: &SandboxProfile,
) -> Result<SandboxAuditRecord, ProtocolErrorBody> {
    let probe_pane_id = PaneId::new();
    let probe_session_id = format!("sandbox-probe-{probe_pane_id}");
    let probe_policy = ResourcePolicy::empty();
    let (handle, _) = spawn_worker(
        context,
        WorkerSpawnSpec {
            session_id: &probe_session_id,
            pane_id: &probe_pane_id,
            program: "/usr/bin/true",
            arguments: &[],
            resource_policy: &probe_policy,
            credential: None,
            sandbox_profile: None,
        },
    )?;
    let result = handle
        .client
        .as_ref()
        .ok_or_else(|| error("worker_unreachable", "sandbox probe worker is unreachable"))
        .and_then(|client| {
            client
                .call(
                    "SANDBOX_PROBE",
                    json!({"sandbox_profile": profile}),
                    SANDBOX_PROBE_TIMEOUT,
                )
                .map_err(map_sandbox_probe_failure)
        });
    if let Some(client) = &handle.client {
        let _ = client.call(
            "SESSION_CLOSE",
            json!({"session_id": probe_session_id}),
            WORKER_MAINTENANCE_TIMEOUT,
        );
    }
    unsafe {
        libc::kill(handle.worker_pid as libc::pid_t, libc::SIGTERM);
    }
    cleanup_worker_scope(&handle);
    let _ = fs::remove_file(&handle.socket);
    let _ = fs::remove_file(&handle.attach_socket);
    sandbox_audit_from_probe(profile, &result?)
}

fn sandbox_audit_from_probe(
    profile: &SandboxProfile,
    result: &Value,
) -> Result<SandboxAuditRecord, ProtocolErrorBody> {
    let backend_path = result
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| error("worker_contract", "sandbox probe has no backend path"))?;
    let backend_version = result
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| error("worker_contract", "sandbox probe has no backend version"))?;
    let probe_time_ms = result
        .get("probe_time_ms")
        .and_then(Value::as_i64)
        .ok_or_else(|| error("worker_contract", "sandbox probe has no probe time"))?;
    let seccomp_profile_hash = result
        .get("seccomp_profile_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| error("worker_contract", "sandbox probe has no seccomp hash"))?;
    let landlock_abi = result
        .get("landlock_abi")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let fd_bind = result.get("fd_bind").and_then(Value::as_bool);
    let profile_json =
        serde_json::to_vec(profile).map_err(|failure| error("internal", failure.to_string()))?;
    Ok(SandboxAuditRecord {
        profile_hash: format!("{:x}", Sha256::digest(profile_json)),
        backend_path: backend_path.into(),
        backend_version: backend_version.into(),
        probe_time_ms,
        seccomp_profile_hash: Some(seccomp_profile_hash.into()),
        landlock_abi,
        fd_bind,
    })
}

fn map_sandbox_probe_failure(failure: ProtocolErrorBody) -> ProtocolErrorBody {
    if matches!(
        failure.code.as_str(),
        "sandbox_profile_unsupported" | "unsupported_platform"
    ) {
        failure
    } else {
        ProtocolErrorBody {
            code: "sandbox_backend_unavailable".into(),
            message: format!("refusing sandboxed capability: {}", failure.message),
            details: failure.details,
        }
    }
}

fn redact_sandbox_error(mut failure: ProtocolErrorBody, full: bool) -> ProtocolErrorBody {
    if !full
        && let Some(details) = failure.details.as_mut()
        && let Some(diagnostics) = details.diagnostics.as_object_mut()
    {
        diagnostics
            .retain(|key, _| matches!(key.as_str(), "environment" | "failed_stage" | "target"));
    }
    failure
}

fn redact_sandbox_value(value: &mut Value, full: bool) {
    if full {
        return;
    }
    if let Some(diagnostics) = value.get_mut("diagnostics").and_then(Value::as_object_mut) {
        diagnostics
            .retain(|key, _| matches!(key.as_str(), "environment" | "failed_stage" | "target"));
    }
}

fn security_capability_issue(
    context: &Context,
    request: &RequestEnvelope,
) -> Result<Value, ProtocolErrorBody> {
    let _security_guard = context
        .security_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let principal_id = request
        .payload
        .get("principal_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing principal_id"))?
        .parse::<PrincipalId>()
        .map_err(|failure| error("invalid_request", failure.to_string()))?;
    let operation = request
        .payload
        .get("operation")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing capability operation"))?
        .parse::<CapabilityOperation>()
        .map_err(|failure| error("invalid_request", failure))?;
    let scope = serde_json::from_value::<CapabilityScope>(
        request
            .payload
            .get("scope")
            .cloned()
            .ok_or_else(|| error("invalid_request", "missing capability scope"))?,
    )
    .map_err(|failure| error("invalid_request", failure.to_string()))?;
    let sandbox_profile = request
        .payload
        .get("sandbox_profile")
        .cloned()
        .map(serde_json::from_value::<SandboxProfile>)
        .transpose()
        .map_err(|failure| error("invalid_request", failure.to_string()))?;
    security::validate_sandbox_grant(operation, sandbox_profile.as_ref())?;
    let diagnostics_trusted = {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        security::require_management(&store, request.auth_context.as_ref(), now_ms())?
            .principal
            .high_trust
    };
    if operation == CapabilityOperation::PaneInput && !diagnostics_trusted {
        let sandboxed_target = match &scope {
            CapabilityScope::Pane { pane_id } => context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .find_pane_sandbox(pane_id)
                .map_err(storage_error)?
                .is_some(),
            _ => false,
        };
        if !sandboxed_target {
            return Err(error(
                "insufficient_trust",
                "pane_input for an unsandboxed Pane requires a high-trust issuer",
            ));
        }
    }
    let sandbox_grant = if let Some(profile) = &sandbox_profile {
        let audit = match operation {
            CapabilityOperation::AgentExec => {
                let pane_id = match &scope {
                    CapabilityScope::Pane { pane_id } => pane_id,
                    _ => {
                        return Err(error(
                            "invalid_request",
                            "agent_exec sandbox_profile requires a Pane scope",
                        ));
                    }
                };
                probe_sandbox_worker(context, pane_id, profile)
            }
            CapabilityOperation::SandboxedPane => probe_sandbox_profile(context, profile),
            _ => unreachable!("sandbox profile validation rejected other operations"),
        };
        let audit = audit.map_err(|failure| redact_sandbox_error(failure, diagnostics_trusted))?;
        if operation == CapabilityOperation::SandboxedPane && audit.fd_bind != Some(true) {
            let failure = error(
                "sandbox_backend_unavailable",
                "sandboxed Pane requires Bubblewrap FD-binding support",
            )
            .with_details(
                "fd_bind_unavailable",
                json!({
                    "required_feature": "--bind-fd/--ro-bind-fd",
                    "backend_version": audit.backend_version
                }),
            );
            return Err(redact_sandbox_error(failure, diagnostics_trusted));
        }
        Some(security::SandboxCapabilityGrant {
            profile: profile.clone(),
            audit,
        })
    } else {
        None
    };
    let ttl_ms = request
        .payload
        .get("ttl_ms")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_CAPABILITY_TTL_MS);
    let issued = {
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        security::require_management(&store, request.auth_context.as_ref(), now_ms())?;
        match &scope {
            CapabilityScope::Pane { pane_id } => {
                if store.find_pane(pane_id).map_err(storage_error)?.is_none() {
                    return Err(error("not_found", "capability target Pane was not found"));
                }
            }
            CapabilityScope::Command {
                pane_id,
                command_id,
            } => {
                let command = store
                    .find_agent_command(command_id)
                    .map_err(storage_error)?
                    .ok_or_else(|| error("not_found", "capability target command was not found"))?;
                if &command.pane_id != pane_id {
                    return Err(error(
                        "invalid_request",
                        "capability command does not belong to the requested Pane",
                    ));
                }
            }
            CapabilityScope::Global | CapabilityScope::Session { .. } => {}
        }
        security::issue_capability(
            &mut store,
            &principal_id,
            operation,
            scope,
            sandbox_grant,
            ttl_ms,
            now_ms(),
        )?
    };
    Ok(security::capability_json(&issued))
}

fn security_principal_revoke(
    context: &Context,
    request: &RequestEnvelope,
) -> Result<Value, ProtocolErrorBody> {
    let principal_id = request
        .payload
        .get("principal_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", "missing principal_id"))?
        .parse::<PrincipalId>()
        .map_err(|failure| error("invalid_request", failure.to_string()))?;
    if principal_id == PrincipalId::local_root() {
        return Err(error(
            "invalid_request",
            "local-root Principal cannot be revoked from the local control API",
        ));
    }
    let severity = request
        .payload
        .get("severity")
        .and_then(Value::as_str)
        .map(str::parse::<RevocationSeverity>)
        .transpose()
        .map_err(|failure| error("invalid_request", failure))?
        .unwrap_or_default();
    let hold = request
        .payload
        .get("hold")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let grace_ms = request
        .payload
        .get("grace_ms")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_FREEZE_GRACE_MS);
    if grace_ms <= 0 || grace_ms > MAX_FREEZE_GRACE_MS {
        return Err(error(
            "invalid_request",
            format!("grace_ms must be between 1 and {MAX_FREEZE_GRACE_MS}"),
        ));
    }
    let revoked = {
        let mut store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let grant = security::require_management(&store, request.auth_context.as_ref(), now_ms())?;
        if (severity == RevocationSeverity::Administrative || hold) && !grant.principal.high_trust {
            return Err(error(
                "insufficient_trust",
                "administrative revocation and security holds require a high-trust Principal",
            ));
        }
        if store
            .find_principal(&principal_id)
            .map_err(storage_error)?
            .is_some_and(|principal| principal.high_trust && !grant.principal.high_trust)
        {
            return Err(error(
                "insufficient_trust",
                "only a high-trust Principal may revoke another high-trust Principal",
            ));
        }
        let deadline_ms =
            (severity == RevocationSeverity::Freeze).then_some(now_ms().saturating_add(grace_ms));
        store
            .revoke_principal(&principal_id, severity, deadline_ms, hold, now_ms())
            .map_err(storage_error)?
    };
    let affected_commands = apply_revocation_to_commands(
        context,
        &principal_id,
        severity,
        revoked.revocation_deadline_ms,
    )?;
    Ok(json!({
        "principal": principal_json(&revoked),
        "affected_commands": affected_commands
    }))
}

fn apply_revocation_to_commands(
    context: &Context,
    principal_id: &PrincipalId,
    severity: RevocationSeverity,
    deadline_ms: Option<i64>,
) -> Result<usize, ProtocolErrorBody> {
    if severity == RevocationSeverity::Administrative {
        return Ok(0);
    }
    let commands = {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let principals = store.list_principals().map_err(storage_error)?;
        let affected = principals
            .into_iter()
            .filter_map(|principal| {
                store
                    .principal_chain(&principal.id)
                    .ok()
                    .filter(|chain| chain.iter().any(|ancestor| &ancestor.id == principal_id))
                    .map(|_| principal.id)
            })
            .collect::<Vec<_>>();
        let mut commands = Vec::new();
        for affected_principal in affected {
            commands.extend(
                store
                    .list_agent_commands_for_principal(&affected_principal)
                    .map_err(storage_error)?
                    .into_iter()
                    .filter(|command| !command.state.is_terminal()),
            );
        }
        commands
    };
    let mut affected = 0;
    for command in commands {
        let operation = if severity == RevocationSeverity::Freeze {
            "AGENT_COMMAND_FREEZE"
        } else {
            "AGENT_COMMAND_TERMINATE"
        };
        let payload = json!({
            "command_id": command.id,
            "reason": "principal_revocation",
            "severity": severity.as_str(),
            "deadline_ms": deadline_ms
        });
        match call_pane_worker(
            context,
            &command.pane_id.to_string(),
            operation,
            payload,
            WORKER_REQUEST_TIMEOUT,
        ) {
            Ok(status) => {
                let pane = context
                    .store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .find_pane(&command.pane_id)
                    .map_err(storage_error)?;
                if let Some(pane) = pane {
                    persist_agent_status(context, &pane, &status)?;
                }
                affected += 1;
            }
            Err(failure) if matches!(failure.code.as_str(), "not_found" | "terminal_lost") => {
                mark_agent_command_lost(context, &command)?;
                affected += 1;
            }
            Err(_) => {}
        }
    }
    Ok(affected)
}

fn reconcile_expired_freeze_revocations(context: &Context) {
    let _security_guard = context
        .security_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let expired = {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        store
            .expired_freeze_principals(now_ms())
            .unwrap_or_default()
    };
    for principal in expired {
        let escalated = {
            let mut store = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            store
                .escalate_principal_revocation(&principal.id, now_ms())
                .ok()
        };
        if let Some(escalated) = escalated {
            let _ = apply_revocation_to_commands(
                context,
                &escalated.id,
                RevocationSeverity::Terminate,
                None,
            );
            eprintln!(
                "security freeze expired; escalated Principal {} to termination",
                escalated.id
            );
        }
    }
}

fn client_operation(
    context: &Context,
    request: &RequestEnvelope,
) -> Result<Value, ProtocolErrorBody> {
    match request.operation.as_str() {
        "PING" => Ok(json!({"daemon": "ok", "protocol": PROTOCOL_VERSION})),
        "CONTROL_STATUS" => control_status(context),
        "SECURITY_BOOTSTRAP" => security_bootstrap(context, request),
        "SECURITY_PRINCIPAL_CREATE" => security_principal_create(context, request),
        "SECURITY_CAPABILITY_ISSUE" => security_capability_issue(context, request),
        "SECURITY_PRINCIPAL_REVOKE" => security_principal_revoke(context, request),
        "AGENT_EXEC" => {
            if request.payload.get("sandbox_profile").is_some() {
                return Err(error(
                    "sandbox_profile_not_client_selectable",
                    "sandbox profiles are selected only by capability issuance",
                ));
            }
            let pane_id = required_pane_id(&request.payload)?;
            let (pane, grant) = {
                let store = context
                    .store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let grant = security::require_agent_exec(
                    &store,
                    request.auth_context.as_ref(),
                    &pane_id,
                    now_ms(),
                )?;
                let pane = store
                    .find_pane(&pane_id)
                    .map_err(storage_error)?
                    .ok_or_else(|| error("not_found", "Pane not found"))?;
                (pane, grant)
            };
            let mut payload = request.payload.clone();
            let sandbox_profile = grant.capability.sandbox_profile.clone();
            let credential_name = match request.payload.get("credential") {
                Some(value) => Some(
                    value
                        .as_str()
                        .ok_or_else(|| error("invalid_request", "credential must be a string"))?,
                ),
                None => None,
            };
            if let Some(name) = credential_name {
                let secret = context
                    .credentials
                    .read(name)
                    .map_err(|failure| error("credential_unavailable", failure.to_string()))?;
                let broker = start_command_credential_broker(&context.runtime_dir, &secret)
                    .map_err(|failure| error("credential_injection_failed", failure.to_string()))?;
                payload["credential_socket"] = json!(broker);
                payload["redaction_policy"] = json!("credential_exact");
            }
            if let Some(object) = payload.as_object_mut() {
                object.remove("credential");
            }
            if let Some(profile) = sandbox_profile {
                payload["sandbox_profile"] = json!(profile);
            }
            payload["session_id"] = json!(pane.session_id);
            payload["pane_id"] = json!(pane.id);
            let mut status = call_pane_worker(
                context,
                &pane.id.to_string(),
                "AGENT_EXEC",
                payload,
                WORKER_REQUEST_TIMEOUT,
            )?;
            if let Some(audit) = &grant.capability.sandbox_audit
                && let Some(sandbox) = status.get_mut("sandbox").and_then(Value::as_object_mut)
            {
                sandbox.insert("audit".into(), json!(audit));
            }
            persist_agent_exec_status(context, &pane, &status, &grant)?;
            Ok(status)
        }
        "AGENT_COMMAND_STATUS"
        | "AGENT_COMMAND_EVENTS"
        | "AGENT_COMMAND_WAIT_IDLE"
        | "AGENT_COMMAND_TERMINATE"
        | "AGENT_COMMAND_SIGNAL" => agent_command_operation(
            context,
            &request.operation,
            &request.payload,
            request.auth_context.as_ref(),
        ),
        "SESSION_CREATE" => create_session(
            context,
            request.payload.clone(),
            request.auth_context.as_ref(),
        ),
        "PANE_CREATE" => create_pane(
            context,
            request.payload.clone(),
            request.auth_context.as_ref(),
        ),
        "PANE_FOCUS" => pane_focus(context, &request.payload),
        "PANE_REORDER" => pane_reorder(context, &request.payload),
        "PANE_FLOATING_UPDATE" => pane_floating_update(context, &request.payload),
        "PANE_FLOATING_RAISE" => pane_floating_raise(context, &request.payload),
        "TASK_STATUS" => task_status(context, &request.payload),
        "SESSION_LAYOUT" => session_layout(context, &request.payload),
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
        "PANE_CLOSE" => teardown_pane(context, &request.payload, "SESSION_CLOSE", false),
        "PANE_KILL" => teardown_pane(context, &request.payload, "SESSION_KILL", true),
        "SERVER_KILL" => kill_server(context, &request.payload),
        "SESSION_INPUT" => {
            let session_id = required_string(&request.payload, "session_id")?;
            let pane = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .find_pane_for_session(session_id)
                .map_err(storage_error)?
                .ok_or_else(|| error("terminal_lost", "session has no Pane"))?;
            reject_sandboxed_input(context, &pane.id)?;
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
        "PANE_STATUS" | "PANE_ATTACH" => {
            let pane_id = required_pane_id(&request.payload)?;
            let pane_key = pane_id.to_string();
            let pane = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .find_pane(&pane_id)
                .map_err(storage_error)?
                .ok_or_else(|| error("not_found", "Pane not found"))?;
            let operation = if request.operation == "PANE_STATUS" {
                "SESSION_STATUS"
            } else {
                "SESSION_ATTACH"
            };
            let mut status = call_pane_worker(
                context,
                &pane_key,
                operation,
                json!({"session_id": pane.session_id}),
                WORKER_REQUEST_TIMEOUT,
            )?;
            apply_pane_status(context, &pane_id, &status)?;
            annotate_resource_status(&mut status, &pane.resource_policy);
            let sandbox = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .find_pane_sandbox(&pane_id)
                .map_err(storage_error)?;
            annotate_persisted_sandbox(&mut status, sandbox.as_ref());
            Ok(status)
        }
        "SESSION_STATUS" | "SESSION_ATTACH" => {
            let session_id = required_string(&request.payload, "session_id")?;
            let mut status = match call_worker(
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
            let pane = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .find_pane_for_session(session_id)
                .map_err(storage_error)?;
            if let Some(pane) = pane {
                annotate_resource_status(&mut status, &pane.resource_policy);
                let sandbox = context
                    .store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .find_pane_sandbox(&pane.id)
                    .map_err(storage_error)?;
                annotate_persisted_sandbox(&mut status, sandbox.as_ref());
            }
            Ok(status)
        }
        _ => Err(error("unsupported_operation", request.operation.clone())),
    }
}

fn requires_idempotency(operation: &str) -> bool {
    matches!(
        operation,
        "SECURITY_PRINCIPAL_CREATE"
            | "SECURITY_PRINCIPAL_REVOKE"
            | "AGENT_EXEC"
            | "AGENT_COMMAND_TERMINATE"
            | "AGENT_COMMAND_SIGNAL"
            | "SESSION_CREATE"
            | "PANE_CREATE"
            | "PANE_FOCUS"
            | "PANE_REORDER"
            | "PANE_FLOATING_UPDATE"
            | "PANE_FLOATING_RAISE"
            | "SESSION_DETACH"
            | "SESSION_INPUT"
            | "SESSION_RESOURCE_UPDATE"
            | "SESSION_RESIZE"
            | "SESSION_CLOSE"
            | "SESSION_KILL"
            | "PANE_CLOSE"
            | "PANE_KILL"
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
    let capability_hash = request
        .auth_context
        .as_ref()
        .map(|context| ReclaimTokenHash::from_token(&context.capability));
    let encoded = serde_json::to_vec(&(
        &request.operation,
        &request.payload,
        capability_hash.as_ref().map(ReclaimTokenHash::as_str),
    ))
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
    annotate_resource_status(&mut response, &pane.resource_policy);
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
    let security_protected = matches!(
        request.operation.as_str(),
        "AGENT_EXEC"
            | "AGENT_COMMAND_TERMINATE"
            | "AGENT_COMMAND_SIGNAL"
            | "SECURITY_PRINCIPAL_CREATE"
            | "SECURITY_PRINCIPAL_REVOKE"
    );
    let _security_guard = security_protected.then(|| {
        context
            .security_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    });
    if security_protected {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        security::require_live_token(&store, request.auth_context.as_ref(), now_ms())?;
    }
    if !requires_idempotency(&request.operation) {
        return client_operation(context, request);
    }
    let Some(key) = request
        .idempotency_key
        .as_deref()
        .filter(|key| !key.is_empty())
    else {
        return Err(error(
            "invalid_request",
            "mutating operations require an idempotency_key",
        ));
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
    if request.operation == "AGENT_COMMAND_OBSERVE" {
        return proxy_agent_observe(client, context, request);
    }
    let response = ResponseEnvelope {
        version: PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        result: execute_request(context, &request),
    };
    write_frame(&mut client, &response)?;
    Ok(())
}

fn proxy_agent_observe(
    client: UnixStream,
    context: &Context,
    request: RequestEnvelope,
) -> Result<(), Box<dyn std::error::Error>> {
    let command_id: AgentCommandId = request
        .payload
        .get("command_id")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("missing command_id"))?
        .parse()
        .map_err(|failure: uuid::Error| std::io::Error::other(failure.to_string()))?;
    let command = find_agent_command(context, &command_id).map_err(|failure| {
        std::io::Error::other(format!("{}: {}", failure.code, failure.message))
    })?;
    {
        let store = context
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        security::require_command_read(&store, request.auth_context.as_ref(), &command, now_ms())
            .map_err(|failure| {
            std::io::Error::other(format!("{}: {}", failure.code, failure.message))
        })?;
    }
    let mut cursor = request
        .payload
        .get("cursor")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let timeout_ms = request
        .payload
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(AGENT_MAX_WAIT_MS)
        .min(AGENT_MAX_WAIT_MS);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut writer = client.try_clone()?;
    writer.set_write_timeout(Some(Duration::from_millis(250)))?;
    let (sender, receiver) = mpsc::sync_channel::<ResponseEnvelope>(32);
    let request_id = request.request_id.clone();
    let writer_thread = thread::spawn(move || {
        for response in receiver {
            write_frame(&mut writer, &response)?;
        }
        Ok::<(), qwe_protocol::IoFrameError>(())
    });
    let mut sent_initial = false;
    let mut final_message = None;
    loop {
        {
            let store = context
                .store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            security::require_command_read(
                &store,
                request.auth_context.as_ref(),
                &command,
                now_ms(),
            )
            .map_err(|failure| {
                std::io::Error::other(format!("{}: {}", failure.code, failure.message))
            })?;
        }
        let events = match call_pane_worker(
            context,
            &command.pane_id.to_string(),
            "AGENT_COMMAND_EVENTS",
            json!({"command_id": command.id, "cursor": cursor, "limit": 512}),
            WORKER_REQUEST_TIMEOUT,
        ) {
            Ok(events) => events,
            Err(failure) if matches!(failure.code.as_str(), "not_found" | "terminal_lost") => {
                let lost = mark_agent_command_lost(context, &command).map_err(|failure| {
                    std::io::Error::other(format!("{}: {}", failure.code, failure.message))
                })?;
                final_message = Some(json!({
                    "type": "end",
                    "state": "lost",
                    "command_id": lost.id
                }));
                break;
            }
            Err(failure) => {
                return Err(std::io::Error::other(format!(
                    "{}: {}",
                    failure.code, failure.message
                ))
                .into());
            }
        };
        cursor = events
            .get("next_cursor")
            .and_then(Value::as_u64)
            .unwrap_or(cursor);
        let state = events
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let event_values = events
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let should_send = !sent_initial
            || !event_values.is_empty()
            || matches!(state.as_str(), "done" | "failed" | "lost");
        let batch = json!({
            "type": "batch",
            "projection": "read_only",
            "command_id": command.id,
            "state": state,
            "cursor": cursor,
            "events": event_values,
            "replay_gap": events.get("replay_gap").cloned().unwrap_or(Value::Null)
        });
        if should_send {
            sent_initial = true;
            let response = ResponseEnvelope {
                version: PROTOCOL_VERSION,
                request_id: request_id.clone(),
                result: Ok(batch),
            };
            match sender.try_send(response) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(_)) => {
                    final_message = Some(json!({
                        "type": "error",
                        "code": "observer_backpressure",
                        "message": "observer did not drain its bounded queue"
                    }));
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => break,
            }
        }
        if matches!(state.as_str(), "done" | "failed" | "lost") {
            final_message = Some(json!({
                "type": "end",
                "state": state,
                "command_id": command.id
            }));
            break;
        }
        if Instant::now() >= deadline {
            final_message = Some(json!({
                "type": "end",
                "state": state,
                "command_id": command.id,
                "timed_out": true
            }));
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if let Some(message) = final_message {
        let _ = sender.try_send(ResponseEnvelope {
            version: PROTOCOL_VERSION,
            request_id,
            result: Ok(message),
        });
    }
    drop(sender);
    let _ = writer_thread.join();
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

enum ObserverInputEvent {
    Closed,
    Violation(attach::AttachError),
}

fn attachment_hub(context: &Context, pane_id: &str) -> Arc<AttachmentHub> {
    let mut hubs = context
        .attachment_hubs
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(
        hubs.entry(pane_id.to_string())
            .or_insert_with(|| Arc::new(AttachmentHub::new())),
    )
}

fn attachment_output_offset(frame: &attach::ReceivedFrame) -> Option<u64> {
    if frame.frame_type != attach::FrameType::Output || frame.payload.len() < 8 {
        return None;
    }
    Some(u64::from_be_bytes(
        frame.payload[..8]
            .try_into()
            .expect("offset has eight bytes"),
    ))
}

fn serve_observer_attachment(
    mut client: UnixStream,
    context: &Arc<Context>,
    open: attach::AttachOpen,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pane = {
        let store = context
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
        match open.pane_id.as_deref() {
            Some(pane_id) => {
                let pane_id = match PaneId::from_str(pane_id) {
                    Ok(pane_id) => pane_id,
                    Err(failure) => {
                        drop(store);
                        send_attachment_error(
                            &mut client,
                            attach_error("invalid_request", failure.to_string()),
                        );
                        return Ok(());
                    }
                };
                match store.find_pane(&pane_id) {
                    Ok(Some(pane)) if pane.session_id == open.session_id => pane,
                    Ok(_) => {
                        drop(store);
                        send_attachment_error(
                            &mut client,
                            attach_error(
                                "not_found",
                                "Pane does not belong to the requested Session",
                            ),
                        );
                        return Ok(());
                    }
                    Err(failure) => {
                        drop(store);
                        send_attachment_error(
                            &mut client,
                            attach_error("persistence_failure", failure.to_string()),
                        );
                        return Ok(());
                    }
                }
            }
            None => match store.find_pane_for_session(&open.session_id) {
                Ok(Some(pane)) => pane,
                Ok(None) => {
                    drop(store);
                    send_attachment_error(
                        &mut client,
                        attach_error("session_lost", "session has no Pane"),
                    );
                    return Ok(());
                }
                Err(failure) => {
                    drop(store);
                    send_attachment_error(
                        &mut client,
                        attach_error("persistence_failure", failure.to_string()),
                    );
                    return Ok(());
                }
            },
        }
    };
    let pane_id = pane.id.to_string();
    let hub = attachment_hub(context, &pane_id);
    let (observer_id, observer_frames) = hub.subscribe();
    let snapshot = match call_pane_worker(
        context,
        &pane_id,
        "SESSION_ATTACH",
        json!({"session_id": open.session_id}),
        WORKER_REQUEST_TIMEOUT,
    ) {
        Ok(status) => match status
            .get("terminal")
            .cloned()
            .ok_or_else(|| std::io::Error::other("worker snapshot has no terminal state"))
            .and_then(|value| {
                serde_json::from_value::<attach::TerminalSnapshot>(value)
                    .map_err(std::io::Error::other)
            }) {
            Ok(snapshot) => snapshot,
            Err(failure) => {
                hub.unsubscribe(observer_id);
                send_attachment_error(
                    &mut client,
                    attach_error("worker_protocol_error", failure.to_string()),
                );
                return Ok(());
            }
        },
        Err(failure) => {
            hub.unsubscribe(observer_id);
            send_attachment_error(&mut client, attach_error(&failure.code, failure.message));
            return Ok(());
        }
    };
    let accepted = attach::AttachAccepted {
        protocol_version: ATTACH_PROTOCOL_VERSION,
        session_id: open.session_id,
        pane_id: Some(pane_id),
        owner: open.owner,
        generation: 0,
        reclaim_token: secure_attach_token()
            .map_err(|failure| std::io::Error::other(failure.message))?,
        rows: snapshot.rows,
        columns: snapshot.columns,
        stream_offset: snapshot.next_stream_offset,
    };
    if attach::write_json_frame(&mut client, attach::FrameType::AttachAccepted, &accepted)
        .and_then(|()| {
            attach::write_json_frame(&mut client, attach::FrameType::Snapshot, &snapshot)
        })
        .is_err()
    {
        hub.unsubscribe(observer_id);
        return Ok(());
    }

    let input_client = client.try_clone()?;
    let (input_sender, input_receiver) = mpsc::channel();
    thread::spawn(move || {
        let event = match attach::read_frame(&mut { input_client }) {
            Ok(_) => ObserverInputEvent::Violation(attach_error(
                "read_only",
                "observer channel cannot send input or control frames",
            )),
            Err(_) => ObserverInputEvent::Closed,
        };
        let _ = input_sender.send(event);
    });

    let snapshot_offset = snapshot.next_stream_offset;
    loop {
        match observer_frames.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => {
                if attachment_output_offset(&frame).is_some_and(|offset| offset < snapshot_offset) {
                    continue;
                }
                if forward_attachment_frame(&mut client, &frame).is_err() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(event) = input_receiver.try_recv() {
                    match event {
                        ObserverInputEvent::Closed => break,
                        ObserverInputEvent::Violation(failure) => {
                            send_attachment_error(&mut client, failure);
                            break;
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    hub.unsubscribe(observer_id);
    let _ = client.shutdown(std::net::Shutdown::Both);
    Ok(())
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
        return serve_observer_attachment(client, context, open);
    }
    if open.rows == 0 || open.columns == 0 {
        send_attachment_error(
            &mut client,
            attach_error("invalid_request", "terminal dimensions must be positive"),
        );
        return Ok(());
    }

    let (raw_token, generation, worker, pane_id) = {
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
        let pane = match open.pane_id.as_deref() {
            Some(pane_id) => {
                let pane_id = match PaneId::from_str(pane_id) {
                    Ok(pane_id) => pane_id,
                    Err(failure) => {
                        drop(store);
                        send_attachment_error(
                            &mut client,
                            attach_error("invalid_request", failure.to_string()),
                        );
                        return Ok(());
                    }
                };
                match store.find_pane(&pane_id) {
                    Ok(Some(pane)) if pane.session_id == open.session_id => pane,
                    Ok(_) => {
                        drop(store);
                        send_attachment_error(
                            &mut client,
                            attach_error(
                                "not_found",
                                "Pane does not belong to the requested Session",
                            ),
                        );
                        return Ok(());
                    }
                    Err(failure) => {
                        drop(store);
                        send_attachment_error(
                            &mut client,
                            attach_error("persistence_failure", failure.to_string()),
                        );
                        return Ok(());
                    }
                }
            }
            None => match store.find_pane_for_session(&open.session_id) {
                Ok(Some(pane)) => pane,
                Ok(None) => {
                    drop(store);
                    send_attachment_error(
                        &mut client,
                        attach_error("session_lost", "session has no Pane"),
                    );
                    return Ok(());
                }
                Err(failure) => {
                    drop(store);
                    send_attachment_error(
                        &mut client,
                        attach_error("persistence_failure", failure.to_string()),
                    );
                    return Ok(());
                }
            },
        };
        let pane_id = pane.id.clone();
        let mut authority = store
            .get_attachment_authority_for_pane(&pane_id)
            .map_err(|failure| std::io::Error::other(failure.to_string()))?
            .unwrap_or_else(AttachmentAuthority::unattached);
        if authority.state != authority.state_at(now) {
            authority.release();
            store
                .write_attachment_authority_for_pane(&pane_id, &authority, now)
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
            .get(&pane.id.to_string())
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
        )
        .with_pane(pane_id.to_string());
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
        if worker_accepted.pane_id != Some(pane_id.to_string()) {
            let _ = worker.shutdown(std::net::Shutdown::Both);
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error(
                    "worker_contract",
                    "worker accepted a different Pane identity",
                ),
            );
            return Ok(());
        }
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
        if let Err(failure) = store.activate_attachment_for_pane(
            &open.session_id,
            &pane_id,
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
            pane_id: Some(pane_id.to_string()),
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
            let _ = store.write_attachment_authority_for_pane(&pane_id, &released, now_ms());
            drop(store);
            send_attachment_error(
                &mut client,
                attach_error("worker_protocol_error", failure.to_string()),
            );
            return Ok(());
        }
        drop(store);
        (raw_token, next.generation, worker, pane_id)
    };

    let hub = attachment_hub(context, &pane_id.to_string());
    let input_client = client.try_clone()?;
    let input_worker = worker.try_clone()?;
    let input = thread::spawn(move || serve_client_attachment_input(input_client, input_worker));
    let mut worker_reader = worker;
    while let Ok(frame) = attach::read_frame(&mut worker_reader) {
        if forward_attachment_frame(&mut client, &frame).is_err() {
            break;
        }
        hub.broadcast(&frame);
    }
    hub.close_observers();
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
        .get_attachment_authority_for_pane(&pane_id)
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
                    .persist_attachment_hold_for_pane(
                        &pane_id,
                        &open.session_id,
                        authority.owner.as_deref().unwrap_or_default(),
                        authority.generation,
                        authority.deadline_ms.unwrap_or(now),
                        now,
                    )
                    .map_err(|failure| std::io::Error::other(failure.to_string()))?;
            } else {
                store
                    .write_attachment_authority_for_pane(&pane_id, &authority, now)
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

fn prepare_user_manager_environment() {
    #[cfg(target_os = "linux")]
    {
        let runtime_dir = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }));
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() && runtime_dir.is_dir() {
            unsafe {
                std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
            }
        }
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
            && runtime_dir.join("bus").exists()
        {
            unsafe {
                std::env::set_var(
                    "DBUS_SESSION_BUS_ADDRESS",
                    format!("unix:path={}", runtime_dir.join("bus").display()),
                );
            }
        }
    }
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
                SupervisorEvent::Worker(WorkerEvent::ControlClosed { pane_id }) => {
                    let handle = monitor_context
                        .workers
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .get(&pane_id)
                        .cloned();
                    if let Some(handle) = handle {
                        set_worker_health(&handle, WorkerHealth::Unreachable);
                    }
                }
                SupervisorEvent::ChildExited { pid } => {
                    let worker = monitor_context
                        .workers
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .iter()
                        .find(|(_, handle)| handle.worker_pid == pid)
                        .map(|(pane_id, handle)| (pane_id.clone(), Arc::clone(handle)));
                    if let Some((pane_id, handle)) = worker {
                        let _ = mark_pane_lost(&monitor_context, &pane_id);
                        cleanup_worker_scope(&handle);
                        monitor_context
                            .workers
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .remove(&pane_id);
                        remove_worker_files(&monitor_context, &pane_id);
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
    let security_context = Arc::clone(context);
    thread::spawn(move || {
        loop {
            thread::sleep(SECURITY_RECONCILE_INTERVAL);
            reconcile_expired_freeze_revocations(&security_context);
        }
    });
    Ok(())
}

fn startup_context(stage: &str, failure: impl std::fmt::Display) -> Box<dyn std::error::Error> {
    let message = format!("qwed startup failed during {stage}: {failure}");
    eprintln!("{message}");
    Box::new(std::io::Error::other(message))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    install_shutdown_handler()
        .map_err(|failure| startup_context("installing the shutdown handler", failure))?;
    prepare_user_manager_environment();
    let resource_support = qwe_runtime::detect_resource_support();
    let runtime_dir = PathBuf::from(format!("{}.runtime", args.socket));
    let runtime_lock = acquire_runtime_lock(&runtime_dir)
        .map_err(|failure| startup_context("acquiring the runtime lock", failure))?;
    mark_subreaper();
    let store_path = args
        .store
        .unwrap_or_else(|| format!("{}.sqlite", args.socket));
    let mut store = Store::open(&store_path)
        .map_err(|failure| startup_context("opening durable storage", failure))?;
    security::initialize_bootstrap(&mut store, &runtime_dir, now_ms())
        .map_err(|failure| startup_context("initializing security capabilities", failure))?;
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
        resource_support,
        store: Mutex::new(store),
        workers: Mutex::new(HashMap::new()),
        attachment_hubs: Mutex::new(HashMap::new()),
        reconciliation_incidents: Mutex::new(ReconciliationIncidents::default()),
        last_scan_ms: AtomicI64::new(0),
        worker_events: worker_event_tx,
        creation_lock: Mutex::new(()),
        idempotency_lock: Mutex::new(()),
        attachment_lock: Mutex::new(()),
        security_lock: Mutex::new(()),
        credentials: CredentialProvider::new(credentials_dir, keychain_prefix, keychain_path),
        _runtime_lock: runtime_lock,
    });
    let report = startup_reconcile(&context)
        .map_err(|failure| startup_context("reconciling persisted workers", failure))?;
    context
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .enter_reclaim_grace(now_ms(), DEFAULT_RECLAIM_GRACE_MS)
        .map_err(|failure| startup_context("entering attachment reclaim grace", failure))?;
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
    let listener = UnixListener::bind(&args.socket)
        .map_err(|failure| startup_context("binding the control socket", failure))?;
    fs::set_permissions(&args.socket, fs::Permissions::from_mode(0o600))
        .map_err(|failure| startup_context("securing the control socket", failure))?;
    let attach_listener = UnixListener::bind(&attach_path)
        .map_err(|failure| startup_context("binding the attach socket", failure))?;
    fs::set_permissions(&attach_path, fs::Permissions::from_mode(0o600))
        .map_err(|failure| startup_context("securing the attach socket", failure))?;
    attach_listener
        .set_nonblocking(true)
        .map_err(|failure| startup_context("configuring the attach listener", failure))?;

    start_monitor_threads(&context, supervisor_event_rx, supervisor_event_tx)
        .map_err(|failure| startup_context("starting supervisor monitors", failure))?;

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

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[test]
    fn terminal_lost_panes_do_not_repeat_storage_reconciliation() {
        assert!(!pane_requires_loss_update(
            Some("lost"),
            Some(TaskState::Exited)
        ));
        assert!(!pane_requires_loss_update(
            Some("lost"),
            Some(TaskState::Lost)
        ));
        assert!(!pane_requires_loss_update(
            Some("closed"),
            Some(TaskState::Stopped)
        ));
        assert!(!pane_requires_loss_update(
            Some("exited"),
            Some(TaskState::Exited)
        ));
    }

    #[test]
    fn inconsistent_lost_panes_still_reconcile_running_tasks() {
        assert!(pane_requires_loss_update(
            Some("lost"),
            Some(TaskState::Running)
        ));
        assert!(pane_requires_loss_update(
            Some("lost"),
            Some(TaskState::Stopped)
        ));
        assert!(pane_requires_loss_update(Some("lost"), None));
    }

    #[test]
    fn live_or_unknown_workers_require_loss_reconciliation() {
        assert!(pane_requires_loss_update(
            Some("active"),
            Some(TaskState::Running)
        ));
        assert!(pane_requires_loss_update(None, None));
    }

    #[test]
    fn sandbox_probe_preserves_permanent_platform_failures() {
        let unsupported = map_sandbox_probe_failure(error(
            "unsupported_platform",
            "Bubblewrap sandboxing is supported only on Linux",
        ));
        assert_eq!(unsupported.code, "unsupported_platform");

        let temporary = map_sandbox_probe_failure(
            error("sandbox_backend_unavailable", "bubblewrap is unavailable")
                .with_details("mount_denied", json!({"target": "/proc"})),
        );
        assert_eq!(temporary.code, "sandbox_backend_unavailable");
        assert_eq!(temporary.details.unwrap().reason, "mount_denied");
    }
}
