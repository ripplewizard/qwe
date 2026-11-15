use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use qwe_core::{
    AGENT_MAX_WAIT_MS, AGENT_OUTPUT_CHUNK_BYTES, AGENT_REPLAY_LIMIT_BYTES, AgentCommandId,
    AgentCommandState, AgentEvent, AgentEventKind, AgentOutputMode, AgentOutputStream, AgentSignal,
    AgentWaitState, ProcessIdentity, SANDBOX_PROFILE_VERSION, SandboxNetworkMode, SandboxProfile,
    SandboxWorkspaceMode,
};
use qwe_protocol::ProtocolErrorBody;
use qwe_runtime::process_identity;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zeroize::{Zeroize, Zeroizing};

use crate::resources::ResourceController;

const EVENT_ACCOUNTING_BYTES: usize = 128;
const EVENT_PAGE_LIMIT: usize = 512;
const WAIT_POLL: Duration = Duration::from_millis(10);

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

pub struct AgentManager {
    session_id: String,
    pane_id: String,
    worker_identity: ProcessIdentity,
    runtime_dir: PathBuf,
    resources: ResourceController,
    commands: Mutex<HashMap<AgentCommandId, Arc<AgentCommand>>>,
}

struct AgentCommand {
    id: AgentCommandId,
    owner: String,
    output_mode: AgentOutputMode,
    idle_capable: bool,
    sandbox_profile: Option<SandboxProfile>,
    sandbox_risk: Option<SandboxRisk>,
    created_at_ms: i64,
    pty_master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    inner: Mutex<AgentCommandInner>,
    changed: Condvar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SandboxRisk {
    network_inherit: bool,
    writable_workspace: bool,
    credential_exposed: bool,
}

struct AgentCommandInner {
    state: AgentCommandState,
    process_identity: Option<ProcessIdentity>,
    exit_code: Option<i32>,
    exit_signal: Option<String>,
    last_error: Option<String>,
    last_error_details: Option<Value>,
    terminate_requested: bool,
    termination_signal: Option<libc::c_int>,
    freeze_requested: bool,
    revocation_severity: Option<String>,
    revocation_deadline_ms: Option<i64>,
    revocation_reason: Option<String>,
    redaction_applied: bool,
    signal_pgid: Option<u32>,
    sandbox_wrapper_pid: Option<u32>,
    sandbox_child_pid: Option<u32>,
    stdout_cursor: u64,
    stderr_cursor: u64,
    pty_cursor: u64,
    events: AgentEventLog,
}

struct AgentEventLog {
    events: VecDeque<AgentEvent>,
    accounted_bytes: usize,
    next_sequence: u64,
    replay_from: u64,
}

struct CommandLaunch {
    command: Arc<AgentCommand>,
    program: String,
    arguments: Vec<String>,
    cwd: Option<PathBuf>,
    deadline_ms: Option<i64>,
    resources: ResourceController,
    credential_socket: Option<PathBuf>,
    redaction_policy: Option<String>,
    runtime_dir: PathBuf,
    sandbox_profile: Option<SandboxProfile>,
}

struct SpawnedAgent {
    child: Child,
    sandbox_child_pid: Option<u32>,
}

type PtyTopologyReceiver = Receiver<Option<(u32, UnixStream)>>;
type SandboxStatusReceiver = Receiver<Option<UnixStream>>;

struct PtySpawnPlan {
    builder: CommandBuilder,
    status_receiver: Option<SandboxStatusReceiver>,
    seccomp_path: Option<PathBuf>,
}

struct PtySpawnRequest<'a> {
    runtime_dir: &'a Path,
    launcher: PathBuf,
    topology_path: &'a Path,
    credential_broker: Option<&'a Path>,
    program: &'a str,
    arguments: &'a [String],
    cwd: Option<&'a Path>,
    sandbox_profile: Option<&'a SandboxProfile>,
    credential_present: bool,
}

impl AgentEventLog {
    fn new() -> Self {
        Self {
            events: VecDeque::new(),
            accounted_bytes: 0,
            next_sequence: 1,
            replay_from: 1,
        }
    }

    fn append(&mut self, command_id: AgentCommandId, kind: AgentEventKind) -> (u64, AgentEvent) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let event = AgentEvent {
            sequence,
            command_id,
            timestamp_ms: now_ms(),
            kind,
        };
        self.accounted_bytes = self
            .accounted_bytes
            .saturating_add(event_accounted_bytes(&event));
        self.events.push_back(event.clone());
        while self.accounted_bytes > AGENT_REPLAY_LIMIT_BYTES && self.events.len() > 1 {
            let Some(removed) = self.events.pop_front() else {
                break;
            };
            self.accounted_bytes = self
                .accounted_bytes
                .saturating_sub(event_accounted_bytes(&removed));
        }
        if let Some(first) = self.events.front() {
            self.replay_from = first.sequence;
        }
        (sequence, event)
    }
}

fn event_accounted_bytes(event: &AgentEvent) -> usize {
    let data_bytes = match &event.kind {
        AgentEventKind::Output { data_hex, .. } => data_hex.len() / 2,
        _ => 0,
    };
    EVENT_ACCOUNTING_BYTES.saturating_add(data_bytes)
}

impl AgentCommand {
    fn new(
        id: AgentCommandId,
        owner: String,
        output_mode: AgentOutputMode,
        idle_capable: bool,
        sandbox_profile: Option<SandboxProfile>,
        credential_present: bool,
    ) -> Self {
        let created_at_ms = now_ms();
        let sandbox_risk = sandbox_profile.as_ref().map(|profile| SandboxRisk {
            network_inherit: profile.network == SandboxNetworkMode::Inherit,
            writable_workspace: profile.workspace_mode == SandboxWorkspaceMode::ReadWrite,
            credential_exposed: credential_present,
        });
        let command = Self {
            id,
            owner,
            output_mode,
            idle_capable,
            sandbox_profile,
            sandbox_risk,
            created_at_ms,
            pty_master: Mutex::new(None),
            inner: Mutex::new(AgentCommandInner {
                state: AgentCommandState::Starting,
                process_identity: None,
                exit_code: None,
                exit_signal: None,
                last_error: None,
                last_error_details: None,
                terminate_requested: false,
                termination_signal: None,
                freeze_requested: false,
                revocation_severity: None,
                revocation_deadline_ms: None,
                revocation_reason: None,
                redaction_applied: false,
                signal_pgid: None,
                sandbox_wrapper_pid: None,
                sandbox_child_pid: None,
                stdout_cursor: 0,
                stderr_cursor: 0,
                pty_cursor: 0,
                events: AgentEventLog::new(),
            }),
            changed: Condvar::new(),
        };
        let command_id = command.id.clone();
        command.with_inner(|inner| {
            inner.events.append(
                command_id,
                AgentEventKind::CommandAccepted {
                    state: AgentCommandState::Starting,
                },
            );
            if let Some(risk) = command.sandbox_risk
                && (risk.network_inherit || risk.writable_workspace || risk.credential_exposed)
            {
                inner.events.append(
                    command.id.clone(),
                    AgentEventKind::SandboxRiskObserved {
                        network_inherit: risk.network_inherit,
                        writable_workspace: risk.writable_workspace,
                        credential_exposed: risk.credential_exposed,
                    },
                );
            }
        });
        command
    }

    fn with_inner<T>(&self, action: impl FnOnce(&mut AgentCommandInner) -> T) -> T {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        action(&mut inner)
    }

    fn mark_started(
        &self,
        identity: ProcessIdentity,
        sandbox_processes: Option<(u32, u32)>,
        signal_pgid: Option<u32>,
    ) {
        let (terminate_requested, freeze_requested) = self.with_inner(|inner| {
            if !inner.state.can_transition_to(AgentCommandState::Running) {
                return (false, false);
            }
            inner.state = AgentCommandState::Running;
            inner.process_identity = Some(identity.clone());
            inner.signal_pgid = signal_pgid;
            inner.sandbox_wrapper_pid = sandbox_processes.map(|(wrapper, _)| wrapper);
            inner.sandbox_child_pid = sandbox_processes.map(|(_, child)| child);
            inner.events.append(
                self.id.clone(),
                AgentEventKind::ProcessStarted {
                    process_identity: identity,
                },
            );
            (inner.terminate_requested, inner.freeze_requested)
        });
        if terminate_requested {
            self.signal(libc::SIGKILL);
        } else if freeze_requested {
            self.signal(libc::SIGSTOP);
        }
        self.changed.notify_all();
    }

    fn push_output(&self, stream: AgentOutputStream, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.with_inner(|inner| {
            if inner.state.is_terminal() {
                return;
            }
            let cursor = match stream {
                AgentOutputStream::Stdout => &mut inner.stdout_cursor,
                AgentOutputStream::Stderr => &mut inner.stderr_cursor,
                AgentOutputStream::Pty => &mut inner.pty_cursor,
            };
            let start = *cursor;
            *cursor = cursor.saturating_add(data.len() as u64);
            inner.events.append(
                self.id.clone(),
                AgentEventKind::Output {
                    stream,
                    cursor: start,
                    data_hex: hex_encode(data),
                },
            );
        });
    }

    fn mark_redaction(&self, policy: &str) {
        self.with_inner(|inner| {
            if inner.redaction_applied {
                return;
            }
            inner.redaction_applied = true;
            inner.events.append(
                self.id.clone(),
                AgentEventKind::RedactionApplied {
                    policy: policy.to_string(),
                },
            );
        });
    }

    fn finish(&self, exit_code: Option<i32>, exit_signal: Option<String>) {
        self.with_inner(|inner| {
            if inner.state.is_terminal() {
                return;
            }
            let exit_signal = exit_signal.or_else(|| {
                inner
                    .termination_signal
                    .map(signal_name)
                    .or_else(|| inner.terminate_requested.then(|| "SIGTERM".into()))
            });
            let state = if inner.terminate_requested || exit_signal.is_some() {
                AgentCommandState::Failed
            } else {
                AgentCommandState::Done
            };
            let previous = inner.state;
            inner.state = state;
            inner.exit_code = exit_code;
            inner.exit_signal = exit_signal.clone();
            inner.events.append(
                self.id.clone(),
                AgentEventKind::StateChanged { previous, state },
            );
            inner.events.append(
                self.id.clone(),
                AgentEventKind::TerminalResult {
                    exit_code,
                    exit_signal,
                    error: None,
                },
            );
        });
        self.changed.notify_all();
    }

    fn fail(&self, reason: String) {
        self.fail_with_details(reason, None);
    }

    fn fail_with_details(&self, reason: String, details: Option<Value>) {
        self.with_inner(|inner| {
            if inner.state.is_terminal() {
                return;
            }
            let previous = inner.state;
            inner.state = AgentCommandState::Failed;
            inner.last_error = Some(reason.clone());
            inner.last_error_details = details.clone();
            inner.events.append(
                self.id.clone(),
                AgentEventKind::StateChanged {
                    previous,
                    state: AgentCommandState::Failed,
                },
            );
            inner.events.append(
                self.id.clone(),
                AgentEventKind::TerminalResult {
                    exit_code: None,
                    exit_signal: None,
                    error: Some(reason),
                },
            );
        });
        self.changed.notify_all();
    }

    fn fail_sandbox_start(&self, reason: String) {
        #[cfg(target_os = "linux")]
        let details = self.sandbox_profile.as_ref().and_then(|_| {
            crate::sandbox::probe_bubblewrap().err().map(|failure| {
                json!({
                    "reason": failure.details.reason,
                    "diagnostics": failure.details.diagnostics
                })
            })
        });
        #[cfg(not(target_os = "linux"))]
        let details = self.sandbox_profile.as_ref().map(|_| {
            json!({
                "reason": "unsupported_platform",
                "diagnostics": {"platform": std::env::consts::OS}
            })
        });
        self.fail_with_details(reason, details);
    }

    fn signal(&self, signal: libc::c_int) -> bool {
        let requested = self.with_inner(|inner| {
            let live_foreground = (self.sandbox_profile.is_some()
                && self.output_mode == AgentOutputMode::PtyMerged)
                .then(|| self.foreground_pty_pgid())
                .flatten();
            let primary = live_foreground.or(inner.signal_pgid).or_else(|| {
                inner
                    .process_identity
                    .as_ref()
                    .map(|identity| identity.pgid)
            });
            primary.map(|primary| (primary, inner.sandbox_wrapper_pid))
        });
        let Some((pgid, fallback)) = requested else {
            return false;
        };
        signal_process_group(pgid, signal)
            || fallback
                .is_some_and(|fallback| fallback != pgid && signal_process_group(fallback, signal))
    }

    fn request_termination(&self, signal: libc::c_int) -> bool {
        let requested = self.with_inner(|inner| {
            if inner.state.is_terminal() {
                return None;
            }
            inner.terminate_requested = true;
            inner.termination_signal = Some(signal);
            let live_foreground = (self.sandbox_profile.is_some()
                && self.output_mode == AgentOutputMode::PtyMerged)
                .then(|| self.foreground_pty_pgid())
                .flatten();
            let primary = live_foreground.or(inner.signal_pgid).or_else(|| {
                inner
                    .process_identity
                    .as_ref()
                    .map(|identity| identity.pgid)
            });
            Some((primary?, inner.sandbox_wrapper_pid))
        });
        let Some((pgid, fallback)) = requested else {
            return false;
        };
        signal_process_group(pgid, signal)
            || fallback
                .is_some_and(|fallback| fallback != pgid && signal_process_group(fallback, signal))
    }

    fn clear_pty_master(&self) {
        self.pty_master
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }

    fn foreground_pty_pgid(&self) -> Option<u32> {
        self.pty_master
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|master| master.as_raw_fd())
            .and_then(|descriptor| {
                let pgid = unsafe { libc::tcgetpgrp(descriptor) };
                (pgid > 0).then_some(pgid as u32)
            })
    }

    fn status_json(
        &self,
        session_id: &str,
        pane_id: &str,
        worker_identity: &ProcessIdentity,
    ) -> Value {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.refresh_idle_state(&mut inner);
        let next_cursor = inner.events.next_sequence.saturating_sub(1);
        json!({
            "command_id": self.id,
            "session_id": session_id,
            "pane_id": pane_id,
            "owner": self.owner,
            "worker_identity": worker_identity,
            "process_identity": inner.process_identity,
            "sandbox": self.sandbox_profile.as_ref().map(|profile| json!({
                "enforced": true,
                "profile": profile,
                "risk": self.sandbox_risk.map(|risk| json!({
                    "network_inherit": risk.network_inherit,
                    "writable_workspace": risk.writable_workspace,
                    "credential_exposed": risk.credential_exposed,
                    "credential_network_exfiltration": risk.network_inherit && risk.credential_exposed,
                    "credential_workspace_persistence": risk.writable_workspace && risk.credential_exposed
                })),
                "setuid_denied": true,
                "no_new_privs": true,
                "signal_pgid": inner.signal_pgid,
                "wrapper_pid": inner.sandbox_wrapper_pid,
                "child_pid": inner.sandbox_child_pid
            })),
            "state": inner.state.as_str(),
            "output_mode": self.output_mode.as_str(),
            "created_at_ms": self.created_at_ms,
            "updated_at_ms": now_ms(),
            "exit_code": inner.exit_code,
            "exit_signal": inner.exit_signal,
            "error": inner.last_error,
            "error_details": inner.last_error_details,
            "revocation": inner.revocation_severity.as_ref().map(|severity| json!({
                "severity": severity,
                "deadline_ms": inner.revocation_deadline_ms,
                "reason": inner.revocation_reason
            })),
            "stdout_cursor": inner.stdout_cursor,
            "stderr_cursor": inner.stderr_cursor,
            "pty_cursor": inner.pty_cursor,
            "next_cursor": next_cursor,
            "replay_from": inner.events.replay_from
        })
    }

    fn events_json(&self, cursor: u64, limit: usize) -> Value {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let replay_from = inner.events.replay_from;
        let replay_gap = cursor.saturating_add(1) < replay_from;
        let events = inner
            .events
            .events
            .iter()
            .filter(|event| event.sequence > cursor)
            .take(limit.min(EVENT_PAGE_LIMIT))
            .cloned()
            .collect::<Vec<_>>();
        let next_cursor = events.last().map_or(cursor, |event| event.sequence);
        json!({
            "command_id": self.id,
            "state": inner.state.as_str(),
            "cursor": cursor,
            "next_cursor": next_cursor,
            "replay_from": replay_from,
            "replay_gap": replay_gap.then(|| json!({
                "from_sequence": cursor.saturating_add(1),
                "to_sequence": replay_from.saturating_sub(1)
            })),
            "events": events
        })
    }

    fn wait_json(&self, timeout_ms: u64) -> Value {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(AGENT_MAX_WAIT_MS));
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some(wait_state) = AgentWaitState::from_command_state(inner.state) {
                return json!({
                    "command_id": self.id,
                    "state": inner.state.as_str(),
                    "wait_state": wait_state,
                    "exit_code": inner.exit_code,
                    "exit_signal": inner.exit_signal,
                    "error": inner.last_error.clone(),
                    "error_details": inner.last_error_details.clone(),
                    "timed_out": false
                });
            }
            if self.refresh_idle_state(&mut inner) {
                return json!({
                    "command_id": self.id,
                    "state": "idle",
                    "wait_state": AgentWaitState::Idle,
                    "exit_code": Option::<i32>::None,
                    "exit_signal": Option::<String>::None,
                    "error": Option::<String>::None,
                    "error_details": Option::<Value>::None,
                    "timed_out": false
                });
            }
            let now = Instant::now();
            if now >= deadline {
                return json!({
                    "command_id": self.id,
                    "state": inner.state.as_str(),
                    "wait_state": AgentWaitState::Unknown,
                    "exit_code": inner.exit_code,
                    "exit_signal": inner.exit_signal,
                    "error": inner.last_error.clone(),
                    "error_details": inner.last_error_details.clone(),
                    "timed_out": true
                });
            }
            let (next, result) = self
                .changed
                .wait_timeout(
                    inner,
                    deadline.saturating_duration_since(now).min(WAIT_POLL),
                )
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner = next;
            if result.timed_out() && inner.state.is_terminal() {
                return json!({
                    "command_id": self.id,
                    "state": inner.state.as_str(),
                    "wait_state": AgentWaitState::from_command_state(inner.state),
                    "exit_code": inner.exit_code,
                    "exit_signal": inner.exit_signal,
                    "error": inner.last_error.clone(),
                    "error_details": inner.last_error_details.clone(),
                    "timed_out": false
                });
            }
        }
    }

    fn wait_for_terminal(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if inner.state.is_terminal() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, _) = self
                .changed
                .wait_timeout(
                    inner,
                    deadline.saturating_duration_since(now).min(WAIT_POLL),
                )
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner = next;
        }
    }

    fn refresh_idle_state(&self, inner: &mut AgentCommandInner) -> bool {
        if !self.idle_capable {
            return false;
        }
        let Some(expected_pgid) = inner.signal_pgid.or_else(|| {
            inner
                .process_identity
                .as_ref()
                .map(|identity| identity.pgid)
        }) else {
            return false;
        };
        let foreground_pgid = self
            .pty_master
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|master| master.as_raw_fd())
            .and_then(|descriptor| {
                let pgid = unsafe { libc::tcgetpgrp(descriptor) };
                (pgid > 0).then_some(pgid as u32)
            });
        let Some(foreground_pgid) = foreground_pgid else {
            return false;
        };
        let idle = foreground_pgid == expected_pgid;
        if idle && inner.state == AgentCommandState::Running {
            inner.state = AgentCommandState::Idle;
            inner.events.append(
                self.id.clone(),
                AgentEventKind::StateChanged {
                    previous: AgentCommandState::Running,
                    state: AgentCommandState::Idle,
                },
            );
            inner
                .events
                .append(self.id.clone(), AgentEventKind::IdleObserved);
            self.changed.notify_all();
            true
        } else if !idle && inner.state == AgentCommandState::Idle {
            inner.state = AgentCommandState::Running;
            inner.events.append(
                self.id.clone(),
                AgentEventKind::StateChanged {
                    previous: AgentCommandState::Idle,
                    state: AgentCommandState::Running,
                },
            );
            self.changed.notify_all();
            false
        } else {
            idle
        }
    }
}

impl AgentManager {
    pub fn new(
        session_id: &str,
        pane_id: &str,
        worker_identity: ProcessIdentity,
        runtime_dir: PathBuf,
        resources: ResourceController,
    ) -> Self {
        Self {
            session_id: session_id.to_string(),
            pane_id: pane_id.to_string(),
            worker_identity,
            runtime_dir,
            resources,
            commands: Mutex::new(HashMap::new()),
        }
    }

    pub fn exec(&self, payload: &Value) -> Result<Value, ProtocolErrorBody> {
        let pty_requested = payload.get("pty").and_then(Value::as_bool).unwrap_or(false);
        if !pty_requested && (payload.get("rows").is_some() || payload.get("columns").is_some()) {
            return Err(error(
                "invalid_request",
                "rows and columns require pty=true",
            ));
        }
        let rows: u16 = if pty_requested {
            payload
                .get("rows")
                .and_then(Value::as_u64)
                .unwrap_or(24)
                .try_into()
                .map_err(|_| error("invalid_request", "rows must be a positive u16"))?
        } else {
            0
        };
        let columns: u16 = if pty_requested {
            payload
                .get("columns")
                .and_then(Value::as_u64)
                .unwrap_or(80)
                .try_into()
                .map_err(|_| error("invalid_request", "columns must be a positive u16"))?
        } else {
            0
        };
        if pty_requested && (rows == 0 || columns == 0) {
            return Err(error(
                "invalid_request",
                "PTY rows and columns must be positive",
            ));
        }
        let sandbox_profile = payload
            .get("sandbox_profile")
            .cloned()
            .map(serde_json::from_value::<SandboxProfile>)
            .transpose()
            .map_err(|failure| error("invalid_request", failure.to_string()))?;
        if let Some(profile) = &sandbox_profile {
            validate_sandbox_profile(profile)?;
        }
        if payload.get("credential").is_some() || payload.get("env").is_some() {
            return Err(error(
                "unsupported_capability",
                "per-command credentials and environment mutation are not enabled in Stage 9.1",
            ));
        }
        let session_id = required_string(payload, "session_id")?;
        if session_id != self.session_id {
            return Err(error(
                "session_mismatch",
                "agent exec target does not match this Pane worker",
            ));
        }
        let pane_id = required_string(payload, "pane_id")?;
        if pane_id != self.pane_id {
            return Err(error(
                "pane_mismatch",
                "agent exec target does not match this Pane worker",
            ));
        }
        let program = required_string(payload, "program")?;
        if program.is_empty() || program.len() > 4096 {
            return Err(error(
                "invalid_request",
                "program must contain between 1 and 4096 bytes",
            ));
        }
        let arguments = payload
            .get("args")
            .cloned()
            .unwrap_or_else(|| json!([]))
            .as_array()
            .ok_or_else(|| error("invalid_request", "args must be an array"))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| error("invalid_request", "every arg must be a string"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cwd = payload
            .get("cwd")
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .ok_or_else(|| error("invalid_request", "cwd must be a non-empty string"))
            })
            .transpose()?;
        let credential_socket = payload
            .get("credential_socket")
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .ok_or_else(|| error("invalid_request", "credential_socket must be a path"))
            })
            .transpose()?;
        let redaction_policy = payload
            .get("redaction_policy")
            .and_then(Value::as_str)
            .map(str::to_string);
        if credential_socket.is_some() != redaction_policy.is_some() {
            return Err(error(
                "invalid_request",
                "credential_socket and redaction_policy must be provided together",
            ));
        }
        if redaction_policy
            .as_deref()
            .is_some_and(|policy| policy != "credential_exact")
        {
            return Err(error(
                "unsupported_capability",
                "only credential_exact redaction is supported",
            ));
        }
        let deadline_ms = payload
            .get("deadline_ms")
            .map(|value| {
                value
                    .as_i64()
                    .ok_or_else(|| error("invalid_request", "deadline_ms must be an integer"))
            })
            .transpose()?;
        let owner = payload
            .get("owner")
            .and_then(Value::as_str)
            .unwrap_or("local")
            .to_string();
        let shell_program = Path::new(&program)
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| matches!(value, "sh" | "bash" | "dash" | "ksh" | "zsh" | "fish"));
        let command_string_mode = arguments
            .iter()
            .any(|argument| argument.starts_with('-') && argument.contains('c'));

        let command = Arc::new(AgentCommand::new(
            AgentCommandId::new(),
            owner,
            if pty_requested {
                AgentOutputMode::PtyMerged
            } else {
                AgentOutputMode::PipeSplit
            },
            pty_requested && shell_program && !command_string_mode,
            sandbox_profile.clone(),
            credential_socket.is_some(),
        ));
        let command_id = command.id.clone();
        self.commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(command_id.clone(), Arc::clone(&command));

        let resources = self.resources.clone();
        let runtime_dir = self.runtime_dir.clone();
        let program = program.to_string();
        let spawned_command = Arc::clone(&command);
        thread::spawn(move || {
            let launch = CommandLaunch {
                command: spawned_command,
                program,
                arguments,
                cwd,
                deadline_ms,
                resources,
                credential_socket,
                redaction_policy,
                runtime_dir,
                sandbox_profile,
            };
            if pty_requested {
                run_pty_command(launch, rows, columns);
            } else {
                run_command(launch);
            }
        });

        Ok(command.status_json(&self.session_id, &self.pane_id, &self.worker_identity))
    }

    pub fn status(&self, command_id: &AgentCommandId) -> Result<Value, ProtocolErrorBody> {
        Ok(self.command(command_id)?.status_json(
            &self.session_id,
            &self.pane_id,
            &self.worker_identity,
        ))
    }

    pub fn events(
        &self,
        command_id: &AgentCommandId,
        cursor: u64,
        limit: usize,
    ) -> Result<Value, ProtocolErrorBody> {
        Ok(self.command(command_id)?.events_json(cursor, limit))
    }

    pub fn wait_idle(
        &self,
        command_id: &AgentCommandId,
        timeout_ms: u64,
    ) -> Result<Value, ProtocolErrorBody> {
        Ok(self.command(command_id)?.wait_json(timeout_ms))
    }

    pub fn freeze(
        &self,
        command_id: &AgentCommandId,
        deadline_ms: Option<i64>,
        reason: &str,
    ) -> Result<Value, ProtocolErrorBody> {
        let command = self.command(command_id)?;
        let event_command_id = command.id.clone();
        command.with_inner(|inner| {
            if inner.state.is_terminal() {
                return;
            }
            inner.revocation_severity = Some("freeze".into());
            inner.revocation_deadline_ms = deadline_ms;
            inner.revocation_reason = Some(reason.into());
            inner.freeze_requested = true;
            inner.events.append(
                event_command_id,
                AgentEventKind::RevocationApplied {
                    severity: "freeze".into(),
                    deadline_ms,
                    reason: reason.into(),
                },
            );
        });
        command.signal(libc::SIGSTOP);
        Ok(command.status_json(&self.session_id, &self.pane_id, &self.worker_identity))
    }

    pub fn terminate_with_context(
        &self,
        command_id: &AgentCommandId,
        severity: &str,
        deadline_ms: Option<i64>,
        reason: &str,
    ) -> Result<Value, ProtocolErrorBody> {
        let command = self.command(command_id)?;
        let event_command_id = command.id.clone();
        command.with_inner(|inner| {
            if inner.state.is_terminal() {
                return;
            }
            inner.revocation_severity = Some(severity.into());
            inner.revocation_deadline_ms = deadline_ms;
            inner.revocation_reason = Some(reason.into());
            inner.events.append(
                event_command_id,
                AgentEventKind::RevocationApplied {
                    severity: severity.into(),
                    deadline_ms,
                    reason: reason.into(),
                },
            );
        });
        let state = command.with_inner(|inner| inner.state);
        if !state.is_terminal() {
            command.request_termination(libc::SIGTERM);
            thread::sleep(Duration::from_millis(100));
            command.request_termination(libc::SIGKILL);
            let _ = command.wait_for_terminal(Duration::from_secs(1));
        }
        Ok(command.status_json(&self.session_id, &self.pane_id, &self.worker_identity))
    }

    pub fn signal(
        &self,
        command_id: &AgentCommandId,
        signal: AgentSignal,
    ) -> Result<Value, ProtocolErrorBody> {
        let command = self.command(command_id)?;
        let state = command.with_inner(|inner| inner.state);
        if state.is_terminal() {
            return Err(error(
                "command_terminal",
                "cannot signal a command in a terminal state",
            ));
        }
        if command.with_inner(|inner| inner.process_identity.is_none()) {
            return Err(error(
                "command_not_started",
                "the command has not established a process identity",
            ));
        }
        if !command.signal(agent_signal_number(signal)) {
            return Err(error(
                "signal_failed",
                "failed to deliver the signal to the command process group",
            ));
        }
        command.with_inner(|inner| {
            inner.events.append(
                command.id.clone(),
                AgentEventKind::SignalDelivered {
                    signal: signal.as_str().into(),
                },
            );
        });
        Ok(command.status_json(&self.session_id, &self.pane_id, &self.worker_identity))
    }

    pub fn terminate_all(&self) {
        let commands = self
            .commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .filter(|command| !command.with_inner(|inner| inner.state.is_terminal()))
            .cloned()
            .collect::<Vec<_>>();
        for command in &commands {
            command.request_termination(libc::SIGTERM);
        }
        thread::sleep(Duration::from_millis(100));
        for command in &commands {
            command.request_termination(libc::SIGKILL);
        }
    }

    fn command(&self, command_id: &AgentCommandId) -> Result<Arc<AgentCommand>, ProtocolErrorBody> {
        self.commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(command_id)
            .cloned()
            .ok_or_else(|| error("not_found", "agent command was not found"))
    }
}

fn required_string<'a>(payload: &'a Value, key: &str) -> Result<&'a str, ProtocolErrorBody> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error("invalid_request", format!("missing {key}")))
}

pub(crate) fn validate_sandbox_profile(profile: &SandboxProfile) -> Result<(), ProtocolErrorBody> {
    if profile.version != SANDBOX_PROFILE_VERSION {
        return Err(error(
            "sandbox_profile_unsupported",
            format!("sandbox profile version {} is unsupported", profile.version),
        ));
    }
    if !profile.seccomp.is_supported_v1() {
        return Err(error(
            "sandbox_profile_unsupported",
            "only the named seccomp profile 'strict' is implemented",
        ));
    }
    if !profile.workspace.is_absolute() || !profile.workspace.is_dir() {
        return Err(error(
            "sandbox_profile_unsupported",
            "sandbox workspace must be an existing absolute directory",
        ));
    }
    if !profile.private_tmp {
        return Err(error(
            "sandbox_profile_unsupported",
            "sandbox profile v1 requires a private /tmp",
        ));
    }
    for path in &profile.protected_paths {
        if !path.is_absolute()
            || !path.starts_with(&profile.workspace)
            || std::fs::symlink_metadata(path).is_err()
        {
            return Err(error(
                "sandbox_profile_unsupported",
                format!(
                    "protected path '{}' must already exist inside the workspace",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn run_command(launch: CommandLaunch) {
    let CommandLaunch {
        command,
        program,
        arguments,
        cwd,
        deadline_ms,
        resources,
        credential_socket,
        redaction_policy,
        runtime_dir: _,
        sandbox_profile,
    } = launch;
    let credential = match credential_socket {
        Some(path) => match receive_credential(&path) {
            Ok(secret) => Some(secret),
            Err(failure) => {
                command.fail(format!("failed to receive command credential: {failure}"));
                return;
            }
        },
        None => None,
    };
    let credential_fd = match credential
        .as_ref()
        .map(|secret| create_credential_pipe(secret))
        .transpose()
    {
        Ok(fd) => fd,
        Err(failure) => {
            command.fail(format!("failed to create credential descriptor: {failure}"));
            return;
        }
    };
    let spawn_result = match sandbox_profile.as_ref() {
        Some(profile) => spawn_sandboxed_process(
            profile,
            &program,
            &arguments,
            cwd.as_deref(),
            &resources,
            credential_fd,
        ),
        None => spawn_agent_process(
            &program,
            &arguments,
            cwd.as_deref(),
            &resources,
            credential_fd,
        ),
    };
    if let Some(descriptor) = credential_fd {
        close_fd(descriptor);
    }
    let SpawnedAgent {
        mut child,
        sandbox_child_pid,
    } = match spawn_result {
        Ok(spawned) => spawned,
        Err(failure) => {
            command.fail_sandbox_start(format!("failed to start command: {failure}"));
            return;
        }
    };
    let identity = match process_identity(child.id()) {
        Ok(identity) => identity,
        Err(failure) => {
            let _ = child.kill();
            let _ = child.wait();
            command.fail(format!("failed to identify process: {failure}"));
            return;
        }
    };
    let sandbox_processes = sandbox_child_pid.map(|child_pid| (identity.pid, child_pid));
    command.mark_started(identity, sandbox_processes, None);

    let (reader_done_sender, reader_done) = mpsc::channel();
    let mut reader_count = 0;
    if let Some(stdout) = child.stdout.take() {
        let command = Arc::clone(&command);
        let reader_done_sender = reader_done_sender.clone();
        let redactor = credential
            .as_ref()
            .zip(redaction_policy.as_deref())
            .map(|(secret, policy)| StreamingRedactor::new(Arc::clone(secret), policy));
        reader_count += 1;
        thread::spawn(move || {
            read_output(stdout, AgentOutputStream::Stdout, command, redactor);
            let _ = reader_done_sender.send(());
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let command = Arc::clone(&command);
        let redactor = credential
            .as_ref()
            .zip(redaction_policy.as_deref())
            .map(|(secret, policy)| StreamingRedactor::new(Arc::clone(secret), policy));
        reader_count += 1;
        thread::spawn(move || {
            read_output(stderr, AgentOutputStream::Stderr, command, redactor);
            let _ = reader_done_sender.send(());
        });
    }

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(failure) => break Err(failure),
        }
        if deadline_ms.is_some_and(|deadline| now_ms() >= deadline) {
            command.signal(libc::SIGKILL);
            match child.wait() {
                Ok(status) => {
                    wait_for_readers(&reader_done, reader_count);
                    command.finish(
                        status.exit_code_checked(),
                        Some("deadline_exceeded".to_string()),
                    );
                    return;
                }
                Err(failure) => {
                    command.fail(format!(
                        "failed to reap deadline-expired process: {failure}"
                    ));
                    return;
                }
            }
        }
        thread::sleep(WAIT_POLL);
    };
    match status {
        Ok(status) => {
            wait_for_readers(&reader_done, reader_count);
            command.finish(status.code(), status.signal().map(signal_name));
        }
        Err(failure) => command.fail(format!("failed to wait for command: {failure}")),
    }
}

fn run_pty_command(launch: CommandLaunch, rows: u16, columns: u16) {
    let CommandLaunch {
        command,
        program,
        arguments,
        cwd,
        deadline_ms,
        resources,
        credential_socket,
        redaction_policy,
        runtime_dir,
        sandbox_profile,
    } = launch;
    let credential = match credential_socket {
        Some(path) => match receive_credential(&path) {
            Ok(secret) => Some(secret),
            Err(failure) => {
                command.fail(format!("failed to receive command credential: {failure}"));
                return;
            }
        },
        None => None,
    };
    let credential_broker = match credential
        .as_ref()
        .map(|secret| start_agent_credential_broker(&runtime_dir, secret))
        .transpose()
    {
        Ok(path) => path,
        Err(failure) => {
            command.fail(format!(
                "failed to create command credential broker: {failure}"
            ));
            return;
        }
    };
    let (topology_path, topology_receiver) = match start_pty_topology_listener(&runtime_dir) {
        Ok(listener) => listener,
        Err(failure) => {
            command.fail(format!("failed to create PTY topology listener: {failure}"));
            return;
        }
    };
    let pair = match native_pty_system().openpty(PtySize {
        rows,
        cols: columns,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(failure) => {
            command.fail(format!("failed to create command PTY: {failure}"));
            return;
        }
    };
    let launcher = match std::env::current_exe() {
        Ok(executable) => executable.with_file_name("qwe-session-exec"),
        Err(failure) => {
            command.fail(format!("failed to resolve command launcher: {failure}"));
            return;
        }
    };
    let PtySpawnPlan {
        builder,
        status_receiver,
        seccomp_path,
    } = match build_pty_spawn_plan(PtySpawnRequest {
        runtime_dir: &runtime_dir,
        launcher,
        topology_path: &topology_path,
        credential_broker: credential_broker.as_deref(),
        program: &program,
        arguments: &arguments,
        cwd: cwd.as_deref(),
        sandbox_profile: sandbox_profile.as_ref(),
        credential_present: credential.is_some(),
    }) {
        Ok(plan) => plan,
        Err(failure) => {
            command.fail(format!("failed to prepare PTY command: {failure}"));
            return;
        }
    };
    let mut child = match pair.slave.spawn_command(builder) {
        Ok(child) => child,
        Err(failure) => {
            if let Some(path) = &seccomp_path {
                let _ = std::fs::remove_file(path);
            }
            command.fail_sandbox_start(format!("failed to start PTY command: {failure}"));
            return;
        }
    };
    drop(pair.slave);
    let (pid, mut topology_stream) = match topology_receiver.recv_timeout(Duration::from_secs(3)) {
        Ok(Some(value)) => value,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(path) = &seccomp_path {
                let _ = std::fs::remove_file(path);
            }
            command.fail_sandbox_start("PTY topology handshake failed".into());
            return;
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(path) = &seccomp_path {
                let _ = std::fs::remove_file(path);
            }
            command.fail_sandbox_start("PTY topology handshake timed out".into());
            return;
        }
    };
    if let Err(failure) = resources.attach_process(pid) {
        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = &seccomp_path {
            let _ = std::fs::remove_file(path);
        }
        command.fail(format!(
            "failed to attach PTY command to Pane workload: {failure}"
        ));
        return;
    }
    if let Err(failure) = topology_stream
        .write_all(b"1")
        .and_then(|()| topology_stream.flush())
    {
        let _ = child.kill();
        let _ = child.wait();
        if let Some(path) = &seccomp_path {
            let _ = std::fs::remove_file(path);
        }
        command.fail(format!("failed to release PTY command: {failure}"));
        return;
    }
    drop(topology_stream);
    let identity = match process_identity(pid) {
        Ok(identity) => identity,
        Err(failure) => {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(path) = &seccomp_path {
                let _ = std::fs::remove_file(path);
            }
            command.fail(format!("failed to identify PTY wrapper: {failure}"));
            return;
        }
    };
    let (sandbox_processes, signal_pgid) = if let Some(status_receiver) = status_receiver {
        let status_stream = match status_receiver.recv_timeout(Duration::from_secs(3)) {
            Ok(Some(stream)) => stream,
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(path) = &seccomp_path {
                    let _ = std::fs::remove_file(path);
                }
                command.fail("Bubblewrap PTY status channel did not connect".into());
                return;
            }
        };
        if let Some(path) = &seccomp_path {
            let _ = std::fs::remove_file(path);
        }
        let child_pid = match await_bubblewrap_startup_stream(status_stream) {
            Ok(child_pid) => child_pid,
            Err(failure) => {
                let _ = child.kill();
                let _ = child.wait();
                command.fail_sandbox_start(format!("Bubblewrap PTY startup failed: {failure}"));
                return;
            }
        };
        (Some((pid, child_pid)), Some(child_pid))
    } else {
        (None, None)
    };
    command.mark_started(identity, sandbox_processes, signal_pgid);
    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(failure) => {
            let _ = child.kill();
            let _ = child.wait();
            command.fail(format!("failed to read command PTY: {failure}"));
            return;
        }
    };
    *command
        .pty_master
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(pair.master);
    let (reader_done_sender, reader_done) = mpsc::channel();
    let reader_command = Arc::clone(&command);
    let redactor = credential
        .as_ref()
        .zip(redaction_policy.as_deref())
        .map(|(secret, policy)| StreamingRedactor::new(Arc::clone(secret), policy));
    thread::spawn(move || {
        read_output(reader, AgentOutputStream::Pty, reader_command, redactor);
        let _ = reader_done_sender.send(());
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(failure) => {
                wait_for_readers(&reader_done, 1);
                command.clear_pty_master();
                command.fail(format!("failed to wait for PTY command: {failure}"));
                return;
            }
        }
        if deadline_ms.is_some_and(|deadline| now_ms() >= deadline) {
            command.signal(libc::SIGKILL);
            break child.wait().ok();
        }
        thread::sleep(WAIT_POLL);
    };
    wait_for_readers(&reader_done, 1);
    command.clear_pty_master();
    match status {
        Some(status) => command.finish(
            status.exit_code().try_into().ok(),
            status.signal().map(str::to_string),
        ),
        None => command.fail("failed to reap PTY command".into()),
    }
}

fn wait_for_readers(readers: &Receiver<()>, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    for _ in 0..count {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if readers.recv_timeout(remaining).is_err() {
            return;
        }
    }
}

#[cfg(target_os = "linux")]
fn spawn_sandboxed_process(
    profile: &SandboxProfile,
    program: &str,
    arguments: &[String],
    cwd: Option<&Path>,
    resources: &ResourceController,
    credential_fd: Option<RawFd>,
) -> std::io::Result<SpawnedAgent> {
    if !resources.commands_in_scope() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Pane worker is outside its hard ResourceScope",
        ));
    }
    let program = resolve_program(program)?;
    let (status_read, status_write) = create_status_pipe()?;
    let seccomp_fd = match crate::sandbox::create_seccomp_filter_fd(&profile.seccomp) {
        Ok(descriptor) => descriptor,
        Err(failure) => {
            close_fd(status_read);
            close_fd(status_write);
            return Err(failure);
        }
    };
    let invocation = match crate::sandbox::bubblewrap_invocation(
        profile,
        &program,
        arguments,
        cwd,
        crate::sandbox::BubblewrapOptions {
            status_fd: status_write,
            seccomp_fd: Some(seccomp_fd),
            credential_present: credential_fd.is_some(),
            term: None,
        },
    ) {
        Ok(invocation) => invocation,
        Err(failure) => {
            close_fd(status_read);
            close_fd(status_write);
            close_fd(seccomp_fd);
            return Err(failure);
        }
    };
    let bubblewrap = match resolve_program(
        invocation
            .executable
            .to_str()
            .ok_or_else(|| std::io::Error::other("bubblewrap path is not UTF-8"))?,
    ) {
        Ok(executable) => executable,
        Err(failure) => {
            close_fd(status_read);
            close_fd(status_write);
            close_fd(seccomp_fd);
            return Err(failure);
        }
    };
    let (release_read, release_write) = match create_release_pipe() {
        Ok(pipe) => pipe,
        Err(failure) => {
            close_fd(status_read);
            close_fd(status_write);
            close_fd(seccomp_fd);
            return Err(failure);
        }
    };
    let launcher = match std::env::current_exe() {
        Ok(executable) => executable.with_file_name("qwe-session-exec"),
        Err(failure) => {
            close_fd(status_read);
            close_fd(status_write);
            close_fd(release_read);
            close_fd(release_write);
            close_fd(seccomp_fd);
            return Err(failure);
        }
    };
    let mut command = Command::new(launcher);
    if let Some(descriptor) = credential_fd {
        command.arg("--credential-fd").arg(descriptor.to_string());
    }
    command
        .arg("--release-fd")
        .arg(release_read.to_string())
        .arg(bubblewrap)
        .args(invocation.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(failure) => {
            close_fd(status_read);
            close_fd(status_write);
            close_fd(release_read);
            close_fd(release_write);
            close_fd(seccomp_fd);
            return Err(failure);
        }
    };
    close_fd(release_read);
    close_fd(status_write);
    close_fd(seccomp_fd);
    if let Err(failure) = resources.attach_process(child.id()) {
        close_fd(status_read);
        close_fd(release_write);
        let _ = child.kill();
        let _ = child.wait();
        return Err(failure);
    }
    let mut release = unsafe { File::from_raw_fd(release_write) };
    if let Err(failure) = release.write_all(b"1").and_then(|()| release.flush()) {
        close_fd(status_read);
        let _ = child.kill();
        let _ = child.wait();
        return Err(failure);
    }
    let sandbox_child_pid = match await_bubblewrap_startup(status_read) {
        Ok(pid) => pid,
        Err(failure) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(failure);
        }
    };
    Ok(SpawnedAgent {
        child,
        sandbox_child_pid: Some(sandbox_child_pid),
    })
}

#[cfg(not(target_os = "linux"))]
fn spawn_sandboxed_process(
    _profile: &SandboxProfile,
    _program: &str,
    _arguments: &[String],
    _cwd: Option<&Path>,
    _resources: &ResourceController,
    _credential_fd: Option<RawFd>,
) -> std::io::Result<SpawnedAgent> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Bubblewrap sandboxing is supported only on Linux",
    ))
}

#[cfg(target_os = "linux")]
fn await_bubblewrap_startup(status_fd: RawFd) -> std::io::Result<u32> {
    await_bubblewrap_startup_reader(unsafe { File::from_raw_fd(status_fd) })
}

#[cfg(target_os = "linux")]
fn await_bubblewrap_startup_stream(stream: UnixStream) -> std::io::Result<u32> {
    await_bubblewrap_startup_reader(stream)
}

#[cfg(target_os = "linux")]
fn await_bubblewrap_startup_reader(stream: impl Read + Send + 'static) -> std::io::Result<u32> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let result = match reader.read_line(&mut line) {
            Ok(0) => Err("Bubblewrap closed its status descriptor before startup".to_string()),
            Ok(_) => serde_json::from_str::<Value>(&line)
                .map_err(|failure| format!("invalid Bubblewrap startup JSON: {failure}"))
                .and_then(|value| {
                    value["child-pid"]
                        .as_u64()
                        .and_then(|pid| u32::try_from(pid).ok())
                        .ok_or_else(|| "Bubblewrap startup JSON has no child-pid".to_string())
                }),
            Err(failure) => Err(format!(
                "failed to read Bubblewrap startup status: {failure}"
            )),
        };
        let _ = sender.send(result);
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    });
    match receiver.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(pid)) => Ok(pid),
        Ok(Err(failure)) => Err(std::io::Error::other(failure)),
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Bubblewrap startup handshake timed out",
        )),
    }
}

fn build_pty_spawn_plan(request: PtySpawnRequest<'_>) -> std::io::Result<PtySpawnPlan> {
    let PtySpawnRequest {
        runtime_dir,
        launcher,
        topology_path,
        credential_broker,
        program,
        arguments,
        cwd,
        sandbox_profile,
        credential_present,
    } = request;
    if let Some(profile) = sandbox_profile {
        #[cfg(target_os = "linux")]
        {
            let (status_path, status_receiver) = start_sandbox_status_listener(runtime_dir)?;
            let seccomp_path =
                runtime_socket_path(runtime_dir, "sandbox-seccomp")?.with_extension("bpf");
            if let Err(failure) =
                crate::sandbox::write_seccomp_filter(&seccomp_path, &profile.seccomp)
            {
                let _ = std::fs::remove_file(&seccomp_path);
                return Err(failure);
            }
            let program = match resolve_program(program) {
                Ok(program) => program,
                Err(failure) => {
                    let _ = std::fs::remove_file(&seccomp_path);
                    return Err(failure);
                }
            };
            let invocation = match crate::sandbox::bubblewrap_invocation(
                profile,
                &program,
                arguments,
                cwd,
                crate::sandbox::BubblewrapOptions {
                    status_fd: 4,
                    seccomp_fd: Some(5),
                    credential_present,
                    term: Some("xterm-256color"),
                },
            ) {
                Ok(invocation) => invocation,
                Err(failure) => {
                    let _ = std::fs::remove_file(&seccomp_path);
                    return Err(failure);
                }
            };
            let bubblewrap = match resolve_program(
                invocation
                    .executable
                    .to_str()
                    .ok_or_else(|| std::io::Error::other("bubblewrap path is not UTF-8"))?,
            ) {
                Ok(executable) => executable,
                Err(failure) => {
                    let _ = std::fs::remove_file(&seccomp_path);
                    return Err(failure);
                }
            };
            let mut builder = CommandBuilder::new(launcher);
            add_pty_launch_arguments(
                &mut builder,
                topology_path,
                credential_broker,
                Some((&status_path, 4)),
                Some((&seccomp_path, 5)),
            );
            builder.arg(bubblewrap);
            builder.args(invocation.arguments);
            return Ok(PtySpawnPlan {
                builder,
                status_receiver: Some(status_receiver),
                seccomp_path: Some(seccomp_path),
            });
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (program, arguments, cwd, credential_present);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Bubblewrap sandboxing is supported only on Linux",
            ));
        }
    }
    let mut builder = CommandBuilder::new(launcher);
    add_pty_launch_arguments(&mut builder, topology_path, credential_broker, None, None);
    builder.arg(program);
    builder.args(arguments.iter());
    builder.env("TERM", "xterm-256color");
    if let Some(cwd) = cwd {
        builder.cwd(cwd);
    }
    Ok(PtySpawnPlan {
        builder,
        status_receiver: None,
        seccomp_path: None,
    })
}

fn add_pty_launch_arguments(
    builder: &mut CommandBuilder,
    topology_path: &Path,
    credential_broker: Option<&Path>,
    status_socket: Option<(&Path, i32)>,
    seccomp_file: Option<(&Path, i32)>,
) {
    builder.arg("--topology-socket");
    builder.arg(topology_path);
    if let Some(path) = credential_broker {
        builder.arg("--credential-socket");
        builder.arg(path);
    }
    if let Some((path, descriptor)) = status_socket {
        builder.arg("--sandbox-status-socket");
        builder.arg(path);
        builder.arg("--sandbox-status-fd");
        builder.arg(descriptor.to_string());
    }
    if let Some((path, descriptor)) = seccomp_file {
        builder.arg("--sandbox-seccomp-file");
        builder.arg(path);
        builder.arg("--sandbox-seccomp-fd");
        builder.arg(descriptor.to_string());
    }
}

fn spawn_agent_process(
    program: &str,
    arguments: &[String],
    cwd: Option<&std::path::Path>,
    resources: &ResourceController,
    credential_fd: Option<RawFd>,
) -> std::io::Result<SpawnedAgent> {
    if !resources.commands_in_scope() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Pane worker is outside its hard ResourceScope",
        ));
    }
    let program = resolve_program(program)?;
    // The helper blocks after exec, allowing spawn() to return before the
    // process is moved into the Pane workload cgroup and released.
    let (read_fd, write_fd) = create_release_pipe()?;
    let launcher = std::env::current_exe()?.with_file_name("qwe-session-exec");
    let mut command = Command::new(launcher);
    if let Some(descriptor) = credential_fd {
        command.arg("--credential-fd").arg(descriptor.to_string());
    }
    command
        .arg("--release-fd")
        .arg(read_fd.to_string())
        .arg(&program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(failure) => {
            close_fd(read_fd);
            close_fd(write_fd);
            return Err(failure);
        }
    };
    close_fd(read_fd);
    if let Err(failure) = resources.attach_process(child.id()) {
        close_fd(write_fd);
        let _ = child.kill();
        let _ = child.wait();
        return Err(failure);
    }
    let mut release = unsafe { File::from_raw_fd(write_fd) };
    if let Err(failure) = release.write_all(b"1").and_then(|()| release.flush()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(failure);
    }
    Ok(SpawnedAgent {
        child,
        sandbox_child_pid: None,
    })
}

fn receive_credential(path: &Path) -> std::io::Result<Arc<Zeroizing<Vec<u8>>>> {
    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut payload = [0_u8; 1];
    let mut iovec = libc::iovec {
        iov_base: payload.as_mut_ptr().cast(),
        iov_len: payload.len(),
    };
    let control_len =
        unsafe { libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as u32) } as usize;
    let mut control = vec![0_u8; control_len];
    let mut message = unsafe { std::mem::zeroed::<libc::msghdr>() };
    message.msg_iov = &mut iovec;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    let received = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, 0) };
    if received < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let header = unsafe { libc::CMSG_FIRSTHDR(&message) };
    if header.is_null()
        || unsafe { (*header).cmsg_level } != libc::SOL_SOCKET
        || unsafe { (*header).cmsg_type } != libc::SCM_RIGHTS
        || unsafe { (*header).cmsg_len }
            < unsafe { libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as u32) } as _
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "credential broker returned no descriptor",
        ));
    }
    let descriptor = unsafe { std::ptr::read(libc::CMSG_DATA(header).cast::<libc::c_int>()) };
    let file = unsafe { File::from_raw_fd(descriptor) };
    let mut secret = Zeroizing::new(Vec::new());
    file.take(16 * 1024 + 1).read_to_end(&mut secret)?;
    if secret.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "command credential is empty",
        ));
    }
    if secret.len() > 16 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "command credential exceeds 16 KiB",
        ));
    }
    Ok(Arc::new(secret))
}

fn create_credential_pipe(secret: &[u8]) -> std::io::Result<RawFd> {
    let mut descriptors = [-1_i32; 2];
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut writer = unsafe { File::from_raw_fd(descriptors[1]) };
    if let Err(failure) = writer.write_all(secret).and_then(|()| writer.flush()) {
        close_fd(descriptors[0]);
        return Err(failure);
    }
    drop(writer);
    Ok(descriptors[0])
}

fn start_agent_credential_broker(
    runtime_dir: &Path,
    secret: &Arc<Zeroizing<Vec<u8>>>,
) -> std::io::Result<PathBuf> {
    let path = runtime_socket_path(runtime_dir, "cred")?;
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let secret = Arc::clone(secret);
    let cleanup_path = path.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.write_all(&secret).and_then(|()| stream.flush());
                    break;
                }
                Err(failure) if failure.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_file(cleanup_path);
    });
    Ok(path)
}

fn start_pty_topology_listener(
    runtime_dir: &Path,
) -> std::io::Result<(PathBuf, PtyTopologyReceiver)> {
    let path = runtime_socket_path(runtime_dir, "pty-topology")?;
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let (sender, receiver) = mpsc::channel();
    let cleanup_path = path.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let result = (|| {
                        let mut reader = BufReader::new(stream.try_clone()?);
                        let mut line = String::new();
                        reader.read_line(&mut line)?;
                        let value: Value =
                            serde_json::from_str(&line).map_err(std::io::Error::other)?;
                        let pid = value
                            .get("pid")
                            .and_then(Value::as_u64)
                            .and_then(|value| u32::try_from(value).ok())
                            .ok_or_else(|| {
                                std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "PTY topology handshake has no pid",
                                )
                            })?;
                        Ok::<_, std::io::Error>(Some((pid, stream)))
                    })();
                    let _ = sender.send(result.ok().flatten());
                    break;
                }
                Err(failure) if failure.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => {
                    let _ = sender.send(None);
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(cleanup_path);
    });
    Ok((path, receiver))
}

fn start_sandbox_status_listener(
    runtime_dir: &Path,
) -> std::io::Result<(PathBuf, SandboxStatusReceiver)> {
    let path = runtime_socket_path(runtime_dir, "sandbox-status")?;
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let (sender, receiver) = mpsc::channel();
    let cleanup_path = path.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = sender.send(Some(stream));
                    break;
                }
                Err(failure) if failure.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => {
                    let _ = sender.send(None);
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(cleanup_path);
    });
    Ok((path, receiver))
}

fn runtime_socket_path(runtime_dir: &Path, prefix: &str) -> std::io::Result<PathBuf> {
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random)
        .map_err(|failure| std::io::Error::other(format!("random source failed: {failure}")))?;
    Ok(runtime_dir.join(format!(
        "{prefix}-{}-{}.sock",
        std::process::id(),
        hex_encode(&random)
    )))
}

fn resolve_program(program: &str) -> std::io::Result<PathBuf> {
    let candidate = PathBuf::from(program);
    if candidate.components().count() > 1 {
        return executable_file(candidate);
    }
    let path = std::env::var_os("PATH").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "PATH is unavailable while resolving program",
        )
    })?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(program);
        if executable_file(candidate.clone()).is_ok() {
            return Ok(candidate);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("program '{program}' was not found on PATH"),
    ))
}

fn executable_file(path: PathBuf) -> std::io::Result<PathBuf> {
    let metadata = std::fs::metadata(&path)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("program '{}' is not executable", path.display()),
        ));
    }
    Ok(path)
}

#[cfg(target_os = "linux")]
fn create_status_pipe() -> std::io::Result<(RawFd, RawFd)> {
    let mut descriptors = [-1_i32; 2];
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut preserved = [-1_i32; 2];
    for (index, descriptor) in descriptors.into_iter().enumerate() {
        preserved[index] = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 10) };
        unsafe {
            libc::close(descriptor);
        }
        if preserved[index] < 0 {
            let failure = std::io::Error::last_os_error();
            for duplicated in preserved {
                if duplicated >= 0 {
                    close_fd(duplicated);
                }
            }
            return Err(failure);
        }
    }
    let flags = unsafe { libc::fcntl(preserved[1], libc::F_GETFD) };
    if flags < 0
        || unsafe { libc::fcntl(preserved[1], libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
    {
        let failure = std::io::Error::last_os_error();
        close_fd(preserved[0]);
        close_fd(preserved[1]);
        return Err(failure);
    }
    Ok((preserved[0], preserved[1]))
}

fn create_release_pipe() -> std::io::Result<(RawFd, RawFd)> {
    let mut descriptors = [-1_i32; 2];
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let read_flags = unsafe { libc::fcntl(descriptors[0], libc::F_GETFD) };
    let write_flags = unsafe { libc::fcntl(descriptors[1], libc::F_GETFD) };
    if read_flags < 0
        || write_flags < 0
        || unsafe {
            libc::fcntl(
                descriptors[0],
                libc::F_SETFD,
                read_flags & !libc::FD_CLOEXEC,
            )
        } < 0
        || unsafe {
            libc::fcntl(
                descriptors[1],
                libc::F_SETFD,
                write_flags | libc::FD_CLOEXEC,
            )
        } < 0
    {
        let failure = std::io::Error::last_os_error();
        close_fd(descriptors[0]);
        close_fd(descriptors[1]);
        return Err(failure);
    }
    Ok((descriptors[0], descriptors[1]))
}

fn close_fd(descriptor: RawFd) {
    if descriptor >= 0 {
        unsafe {
            libc::close(descriptor);
        }
    }
}

struct StreamingRedactor {
    secret: Arc<Zeroizing<Vec<u8>>>,
    policy: String,
    pending: Zeroizing<Vec<u8>>,
}

impl StreamingRedactor {
    fn new(secret: Arc<Zeroizing<Vec<u8>>>, policy: &str) -> Self {
        Self {
            secret,
            policy: policy.to_string(),
            pending: Zeroizing::new(Vec::new()),
        }
    }

    fn feed(&mut self, data: &[u8]) -> (Vec<u8>, bool) {
        let mut combined = Zeroizing::new(self.pending.to_vec());
        combined.extend_from_slice(data);
        let hold = longest_secret_prefix_suffix(&combined, &self.secret);
        let emit_len = combined.len().saturating_sub(hold);
        let (redacted, matched) = replace_exact(&combined[..emit_len], &self.secret);
        *self.pending = combined[emit_len..].to_vec();
        (redacted, matched)
    }

    fn finish(&mut self) -> (Vec<u8>, bool) {
        let hold = longest_secret_prefix_suffix(&self.pending, &self.secret);
        let emit_len = self.pending.len().saturating_sub(hold);
        let (mut redacted, mut matched) = replace_exact(&self.pending[..emit_len], &self.secret);
        if hold > 0 {
            redacted.extend_from_slice(b"[REDACTED]");
            matched = true;
        }
        self.pending.zeroize();
        (redacted, matched)
    }

    fn policy(&self) -> &str {
        &self.policy
    }
}

fn longest_secret_prefix_suffix(data: &[u8], secret: &[u8]) -> usize {
    for length in (1..secret.len()).rev() {
        if data.ends_with(&secret[..length]) {
            return length;
        }
    }
    0
}

fn replace_exact(data: &[u8], secret: &[u8]) -> (Vec<u8>, bool) {
    if secret.is_empty() {
        return (data.to_vec(), false);
    }
    let mut output = Vec::with_capacity(data.len());
    let mut offset = 0;
    let mut matched = false;
    while offset < data.len() {
        if data[offset..].starts_with(secret) {
            output.extend_from_slice(b"[REDACTED]");
            offset += secret.len();
            matched = true;
        } else {
            output.push(data[offset]);
            offset += 1;
        }
    }
    (output, matched)
}

fn read_output(
    mut reader: impl Read,
    stream: AgentOutputStream,
    command: Arc<AgentCommand>,
    mut redactor: Option<StreamingRedactor>,
) {
    let mut buffer = vec![0_u8; AGENT_OUTPUT_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                if let Some(redactor) = redactor.as_mut() {
                    let (output, matched) = redactor.finish();
                    if matched {
                        command.mark_redaction(redactor.policy());
                    }
                    command.push_output(stream, &output);
                }
                buffer.zeroize();
                return;
            }
            Ok(count) => {
                if let Some(redactor) = redactor.as_mut() {
                    let (output, matched) = redactor.feed(&buffer[..count]);
                    if matched {
                        command.mark_redaction(redactor.policy());
                    }
                    command.push_output(stream, &output);
                } else {
                    command.push_output(stream, &buffer[..count]);
                }
            }
            Err(failure) if failure.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                buffer.zeroize();
                return;
            }
        }
    }
}

fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGKILL => "SIGKILL",
        libc::SIGTERM => "SIGTERM",
        libc::SIGINT => "SIGINT",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGABRT => "SIGABRT",
        _ => "UNKNOWN",
    }
    .to_string()
}

fn signal_process_group(pgid: u32, signal: libc::c_int) -> bool {
    let result = unsafe { libc::kill(-(pgid as libc::pid_t), signal) };
    result == 0
}

fn agent_signal_number(signal: AgentSignal) -> libc::c_int {
    match signal {
        AgentSignal::Sigint => libc::SIGINT,
        AgentSignal::Sigterm => libc::SIGTERM,
        AgentSignal::Sighup => libc::SIGHUP,
        AgentSignal::Sigquit => libc::SIGQUIT,
    }
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

trait ExitCodeChecked {
    fn exit_code_checked(&self) -> Option<i32>;
}

impl ExitCodeChecked for std::process::ExitStatus {
    fn exit_code_checked(&self) -> Option<i32> {
        self.code()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_replay_is_bounded_without_renumbering() {
        let command_id = AgentCommandId::new();
        let mut log = AgentEventLog::new();
        let payload = vec![b'x'; AGENT_OUTPUT_CHUNK_BYTES];
        for _ in 0..20 {
            log.append(
                command_id.clone(),
                AgentEventKind::Output {
                    stream: AgentOutputStream::Stdout,
                    cursor: 0,
                    data_hex: hex_encode(&payload),
                },
            );
        }
        assert!(log.accounted_bytes <= AGENT_REPLAY_LIMIT_BYTES);
        assert!(log.replay_from > 1);
        assert_eq!(
            log.events.back().map(|event| event.sequence),
            Some(log.next_sequence - 1)
        );
    }

    #[test]
    fn failed_start_is_terminal_and_visible() {
        let command = AgentCommand::new(
            AgentCommandId::new(),
            "local".into(),
            AgentOutputMode::PipeSplit,
            false,
            None,
            false,
        );
        command.fail("executable not found".into());
        let value = command.status_json(
            "session",
            "pane",
            &ProcessIdentity {
                pid: 1,
                pgid: 1,
                start_token: "test".into(),
            },
        );
        assert_eq!(value["state"], "failed");
        assert_eq!(value["exit_code"], Value::Null);
    }

    #[test]
    fn streaming_redaction_handles_split_and_partial_secrets() {
        let secret = Arc::new(Zeroizing::new(b"supersecret".to_vec()));
        let mut redactor = StreamingRedactor::new(Arc::clone(&secret), "credential_exact");
        let (first, matched_first) = redactor.feed(b"before super");
        assert!(!matched_first);
        assert_eq!(first, b"before ".to_vec());
        let (second, matched_second) = redactor.feed(b"secret after");
        assert!(matched_second);
        let (tail, matched_tail) = redactor.finish();
        assert_eq!(second, b"[REDACTED] after");
        assert!(!matched_tail);
        assert!(tail.is_empty());
        assert!(
            !second
                .windows(secret.len())
                .any(|window| window == secret.as_slice())
        );

        let mut partial = StreamingRedactor::new(
            Arc::new(Zeroizing::new(b"supersecret".to_vec())),
            "credential_exact",
        );
        let (_, matched) = partial.feed(b"super");
        assert!(!matched);
        let (tail, matched) = partial.finish();
        assert!(matched);
        assert_eq!(tail, b"[REDACTED]");
    }
}
