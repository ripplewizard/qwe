use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

mod agent;
mod authority;
mod layout;
mod sandbox;
mod security;

pub use agent::{
    AGENT_MAX_WAIT_MS, AGENT_OUTPUT_CHUNK_BYTES, AGENT_REPLAY_LIMIT_BYTES, AgentCommandId,
    AgentCommandRecord, AgentCommandState, AgentEvent, AgentEventKind, AgentOutputMode,
    AgentOutputStream, AgentWaitState,
};
pub use authority::{
    AttachmentAuthority, AttachmentOperation, AuthorityCredentials, AuthorityError,
    DEFAULT_DETACHED_HOLD_TTL_MS, DEFAULT_LEASE_TTL_MS, DEFAULT_RECLAIM_GRACE_MS, MAX_LEASE_TTL_MS,
    ReclaimTokenHash,
};
pub use layout::{
    LayoutError, LayoutRect, MAX_LAYOUT_PANES, PaneKind, PaneLayout, PaneSpec, tile_pane_specs,
    tile_panes,
};
pub use sandbox::{
    SANDBOX_PROFILE_VERSION, SandboxHomeMode, SandboxNetworkMode, SandboxProcessMode,
    SandboxProfile, SandboxSeccompMode, SandboxWorkspaceMode,
};
pub use security::{
    CapabilityId, CapabilityOperation, CapabilityRecord, CapabilityScope,
    DEFAULT_CAPABILITY_TTL_MS, DEFAULT_FREEZE_GRACE_MS, MAX_CAPABILITY_TTL_MS, MAX_FREEZE_GRACE_MS,
    PrincipalEpoch, RevocationSeverity,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(Uuid);

impl NodeId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for NodeId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(Uuid);

impl TaskId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(Uuid);

impl PaneId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for PaneId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PaneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PaneId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceScopeId(Uuid);

impl ResourceScopeId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for ResourceScopeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ResourceScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ResourceScopeId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrincipalId(String);

impl PrincipalId {
    #[must_use]
    pub fn local_root() -> Self {
        Self("local-root".into())
    }

    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string())
    }
}

impl Default for PrincipalId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PrincipalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PrincipalId {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(value.to_string()))
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for TaskId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub pgid: u32,
    pub start_token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Running,
    Exited,
    Stopped,
    Lost,
}

impl TaskState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Stopped => "stopped",
            Self::Lost => "lost",
        }
    }

    #[must_use]
    pub fn from_db(value: &str) -> Self {
        match value {
            "running" => Self::Running,
            "exited" => Self::Exited,
            "stopped" => Self::Stopped,
            _ => Self::Lost,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Creating,
    Running,
    Exited,
    Lost,
    Closing,
    Closed,
}

impl SessionState {
    pub const ALL: [Self; 6] = [
        Self::Creating,
        Self::Running,
        Self::Exited,
        Self::Lost,
        Self::Closing,
        Self::Closed,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Lost => "lost",
            Self::Closing => "closing",
            Self::Closed => "closed",
        }
    }

    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Creating, Self::Running | Self::Lost)
                | (Self::Running, Self::Exited | Self::Lost | Self::Closing)
                | (Self::Exited, Self::Lost | Self::Closing)
                | (Self::Closing, Self::Closed)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskInstanceState {
    Starting,
    Running,
    Exited,
    Stopped,
    Lost,
}

impl TaskInstanceState {
    pub const ALL: [Self; 5] = [
        Self::Starting,
        Self::Running,
        Self::Exited,
        Self::Stopped,
        Self::Lost,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Stopped => "stopped",
            Self::Lost => "lost",
        }
    }

    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Starting, Self::Running | Self::Stopped | Self::Lost)
                | (Self::Running, Self::Exited | Self::Stopped | Self::Lost)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentAuthorityState {
    Unattached,
    Active,
    ReclaimGrace,
    Held,
}

impl AttachmentAuthorityState {
    pub const ALL: [Self; 4] = [
        Self::Unattached,
        Self::Active,
        Self::ReclaimGrace,
        Self::Held,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unattached => "unattached",
            Self::Active => "active",
            Self::ReclaimGrace => "reclaim_grace",
            Self::Held => "held",
        }
    }

    #[must_use]
    pub fn from_db(value: &str) -> Self {
        match value {
            "active" => Self::Active,
            "reclaim_grace" => Self::ReclaimGrace,
            "held" => Self::Held,
            _ => Self::Unattached,
        }
    }

    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Unattached, Self::Active)
                | (
                    Self::Active,
                    Self::Unattached | Self::ReclaimGrace | Self::Held
                )
                | (Self::ReclaimGrace, Self::Active | Self::Unattached)
                | (Self::Held, Self::Unattached | Self::Active | Self::Held)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerHealth {
    Healthy,
    Unreachable,
    Absent,
}

impl WorkerHealth {
    pub const ALL: [Self; 3] = [Self::Healthy, Self::Unreachable, Self::Absent];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Unreachable => "unreachable",
            Self::Absent => "absent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationDisposition {
    Matched,
    Orphaned,
    Conflict,
}

impl ReconciliationDisposition {
    pub const ALL: [Self; 3] = [Self::Matched, Self::Orphaned, Self::Conflict];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Matched => "matched",
            Self::Orphaned => "orphaned",
            Self::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub owner: String,
    pub state: String,
    pub identity: Option<ProcessIdentity>,
    pub worker_identity: Option<ProcessIdentity>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: TaskId,
    pub session_id: String,
    pub pane_id: PaneId,
    pub identity: ProcessIdentity,
    pub state: TaskState,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<String>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourcePolicy {
    pub memory_high_bytes: Option<u64>,
    #[serde(alias = "memory_bytes")]
    pub memory_max_bytes: Option<u64>,
    pub memory_swap_max_bytes: Option<u64>,
    #[serde(alias = "process_limit")]
    pub tasks_max: Option<u64>,
    pub cpu_quota_percent: Option<u32>,
    pub cpu_seconds: Option<u64>,
}

impl ResourcePolicy {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            memory_high_bytes: None,
            memory_max_bytes: None,
            memory_swap_max_bytes: None,
            tasks_max: None,
            cpu_quota_percent: None,
            cpu_seconds: None,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::empty()
    }
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceProfile {
    Interactive,
    Batch,
    Tight,
    Unlimited,
}

impl ResourceProfile {
    pub const ALL: [Self; 4] = [Self::Interactive, Self::Batch, Self::Tight, Self::Unlimited];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Batch => "batch",
            Self::Tight => "tight",
            Self::Unlimited => "unlimited",
        }
    }

    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Interactive => {
                "interactive terminals with bounded memory, swap, tasks, and CPU quota"
            }
            Self::Batch => "longer compiler or batch jobs with wider resource allowances",
            Self::Tight => "small shell utilities with conservative hard limits",
            Self::Unlimited => "no requested resource limits; explicit fields may still be added",
        }
    }

    #[must_use]
    pub fn policy(self) -> ResourcePolicy {
        match self {
            Self::Interactive => ResourcePolicy {
                memory_high_bytes: Some(1024 * 1024 * 1024),
                memory_max_bytes: Some(2 * 1024 * 1024 * 1024),
                memory_swap_max_bytes: Some(0),
                tasks_max: Some(256),
                cpu_quota_percent: Some(200),
                cpu_seconds: None,
            },
            Self::Batch => ResourcePolicy {
                memory_high_bytes: Some(2 * 1024 * 1024 * 1024),
                memory_max_bytes: Some(4 * 1024 * 1024 * 1024),
                memory_swap_max_bytes: Some(1024 * 1024 * 1024),
                tasks_max: Some(512),
                cpu_quota_percent: Some(400),
                cpu_seconds: None,
            },
            Self::Tight => ResourcePolicy {
                memory_high_bytes: Some(192 * 1024 * 1024),
                memory_max_bytes: Some(256 * 1024 * 1024),
                memory_swap_max_bytes: Some(0),
                tasks_max: Some(64),
                cpu_quota_percent: Some(100),
                cpu_seconds: None,
            },
            Self::Unlimited => ResourcePolicy::empty(),
        }
    }

    #[must_use]
    pub fn classify(policy: &ResourcePolicy) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|profile| profile.policy() == *policy)
    }
}

impl FromStr for ResourceProfile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "interactive" => Ok(Self::Interactive),
            "batch" => Ok(Self::Batch),
            "tight" => Ok(Self::Tight),
            "unlimited" => Ok(Self::Unlimited),
            _ => Err(format!("unknown resource profile '{value}'")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRecord {
    pub id: PaneId,
    pub session_id: String,
    pub resource_scope_id: ResourceScopeId,
    pub resource_policy: ResourcePolicy,
    pub kind: PaneKind,
    pub layout_order: u32,
    pub layout_weight: u16,
    pub z_index: u32,
    pub floating_rect: Option<LayoutRect>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneWorkerRecord {
    pub pane_id: PaneId,
    pub identity: ProcessIdentity,
    pub state: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceScopeTargetKind {
    Session,
    Pane,
    Task,
}

impl ResourceScopeTargetKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Pane => "pane",
            Self::Task => "task",
        }
    }

    #[must_use]
    pub fn from_db(value: &str) -> Self {
        match value {
            "session" => Self::Session,
            "task" => Self::Task,
            _ => Self::Pane,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceBackend {
    LinuxCgroupV2,
    MacosMonitored,
    Logical,
}

impl ResourceBackend {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LinuxCgroupV2 => "linux_cgroup_v2",
            Self::MacosMonitored => "macos_monitored",
            Self::Logical => "logical",
        }
    }

    #[must_use]
    pub fn from_db(value: &str) -> Self {
        match value {
            "linux_cgroup_v2" => Self::LinuxCgroupV2,
            "macos_monitored" => Self::MacosMonitored,
            _ => Self::Logical,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceScopeState {
    Creating,
    Active,
    Closing,
    Closed,
    Lost,
}

impl ResourceScopeState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Active => "active",
            Self::Closing => "closing",
            Self::Closed => "closed",
            Self::Lost => "lost",
        }
    }

    #[must_use]
    pub fn from_db(value: &str) -> Self {
        match value {
            "creating" => Self::Creating,
            "closing" => Self::Closing,
            "closed" => Self::Closed,
            "lost" => Self::Lost,
            _ => Self::Active,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceScopeRecord {
    pub id: ResourceScopeId,
    pub session_id: String,
    pub target_kind: ResourceScopeTargetKind,
    pub target_id: String,
    pub parent_scope_id: Option<ResourceScopeId>,
    pub backend: ResourceBackend,
    pub requested_policy: ResourcePolicy,
    pub lifecycle_state: ResourceScopeState,
    pub cleanup_outcome: Option<String>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalRecord {
    pub id: PrincipalId,
    pub parent_id: Option<PrincipalId>,
    pub epoch: u64,
    pub created_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
    pub high_trust: bool,
    pub revocation_severity: Option<RevocationSeverity>,
    pub revocation_deadline_ms: Option<i64>,
    pub revocation_hold: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub session_id: String,
    pub owner: String,
    pub generation: u64,
    pub deadline_ms: i64,
    pub updated_at_ms: i64,
    pub released_at_ms: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn contract_schema() -> Value {
        serde_json::from_str(include_str!("../../../protocol/schema/qwe-v1.schema.json"))
            .expect("canonical protocol schema is valid JSON")
    }

    fn schema_strings(value: &Value) -> Vec<&str> {
        value
            .as_array()
            .expect("schema field is an array")
            .iter()
            .map(|entry| entry.as_str().expect("schema entry is a string"))
            .collect()
    }

    fn assert_transition_drift<S>(
        schema: &Value,
        states: &[S],
        as_str: impl Fn(S) -> &'static str,
        can_transition: impl Fn(S, S) -> bool,
    ) where
        S: Copy,
    {
        assert_eq!(
            schema_strings(&schema["states"]),
            states.iter().copied().map(&as_str).collect::<Vec<_>>()
        );
        let mut actual = Vec::new();
        for from in states.iter().copied() {
            for to in states.iter().copied() {
                if can_transition(from, to) {
                    actual.push([as_str(from), as_str(to)]);
                }
            }
        }
        let mut expected: Vec<[&str; 2]> = schema["transitions"]
            .as_array()
            .expect("transitions are an array")
            .iter()
            .map(|transition| {
                let pair = transition.as_array().expect("transition is a pair");
                [
                    pair[0].as_str().expect("from state is a string"),
                    pair[1].as_str().expect("to state is a string"),
                ]
            })
            .collect();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn ids_are_time_ordered_and_unique() {
        let first = TaskId::new();
        let second = TaskId::new();
        assert_ne!(first, second);
    }

    #[test]
    fn session_state_machine_rejects_resurrection() {
        assert!(SessionState::Running.can_transition_to(SessionState::Closing));
        assert!(!SessionState::Closed.can_transition_to(SessionState::Running));
        assert!(!SessionState::Lost.can_transition_to(SessionState::Closing));
    }

    #[test]
    fn task_instance_has_one_initial_transition_path() {
        assert!(TaskInstanceState::Starting.can_transition_to(TaskInstanceState::Running));
        assert!(TaskInstanceState::Running.can_transition_to(TaskInstanceState::Stopped));
        assert!(!TaskInstanceState::Exited.can_transition_to(TaskInstanceState::Running));
    }

    #[test]
    fn attachment_authority_has_no_active_hold_overlap() {
        assert!(AttachmentAuthorityState::Active.can_transition_to(AttachmentAuthorityState::Held));
        assert!(
            !AttachmentAuthorityState::ReclaimGrace
                .can_transition_to(AttachmentAuthorityState::Held)
        );
    }

    #[test]
    fn accepted_state_machines_match_protocol_schema() {
        let schema = contract_schema();
        let machines = &schema["x-qwe-state-machines"];
        assert_transition_drift(
            &machines["session"],
            &SessionState::ALL,
            SessionState::as_str,
            SessionState::can_transition_to,
        );
        assert_transition_drift(
            &machines["task_instance"],
            &TaskInstanceState::ALL,
            TaskInstanceState::as_str,
            TaskInstanceState::can_transition_to,
        );
        assert_transition_drift(
            &machines["attachment_authority"],
            &AttachmentAuthorityState::ALL,
            AttachmentAuthorityState::as_str,
            AttachmentAuthorityState::can_transition_to,
        );
        assert_transition_drift(
            &machines["agent_command"],
            &AgentCommandState::ALL,
            AgentCommandState::as_str,
            AgentCommandState::can_transition_to,
        );
        assert_eq!(
            schema_strings(&machines["worker_health"]),
            WorkerHealth::ALL
                .iter()
                .copied()
                .map(WorkerHealth::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            schema_strings(&machines["reconciliation_disposition"]),
            ReconciliationDisposition::ALL
                .iter()
                .copied()
                .map(ReconciliationDisposition::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            schema_strings(&machines["capability_operation"]),
            CapabilityOperation::ALL
                .iter()
                .copied()
                .map(CapabilityOperation::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            schema_strings(&machines["revocation_severity"]),
            RevocationSeverity::ALL
                .iter()
                .copied()
                .map(RevocationSeverity::as_str)
                .collect::<Vec<_>>()
        );
        for state in SessionState::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
        for state in TaskInstanceState::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
        for state in AttachmentAuthorityState::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
        for state in AgentCommandState::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
        for health in WorkerHealth::ALL {
            assert_eq!(serde_json::to_value(health).unwrap(), health.as_str());
        }
        for operation in CapabilityOperation::ALL {
            assert_eq!(serde_json::to_value(operation).unwrap(), operation.as_str());
        }
        for severity in RevocationSeverity::ALL {
            assert_eq!(serde_json::to_value(severity).unwrap(), severity.as_str());
        }
        for disposition in ReconciliationDisposition::ALL {
            assert_eq!(
                serde_json::to_value(disposition).unwrap(),
                disposition.as_str()
            );
        }
    }

    #[test]
    fn resource_profiles_have_stable_names_and_policies() {
        assert_eq!(
            ResourceProfile::ALL.map(ResourceProfile::as_str),
            ["interactive", "batch", "tight", "unlimited"]
        );
        for profile in ResourceProfile::ALL {
            assert_eq!(
                profile.as_str().parse::<ResourceProfile>().unwrap(),
                profile
            );
            assert_eq!(
                serde_json::to_value(profile).unwrap(),
                serde_json::Value::String(profile.as_str().into())
            );
            assert_eq!(ResourceProfile::classify(&profile.policy()), Some(profile));
        }
        assert_eq!(ResourceProfile::Unlimited.policy(), ResourcePolicy::empty());
        assert!("surprise".parse::<ResourceProfile>().is_err());
    }
}
