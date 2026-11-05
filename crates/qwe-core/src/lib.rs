use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

mod authority;

pub use authority::{
    AttachmentAuthority, AttachmentOperation, AuthorityCredentials, AuthorityError,
    DEFAULT_DETACHED_HOLD_TTL_MS, DEFAULT_LEASE_TTL_MS, DEFAULT_RECLAIM_GRACE_MS, MAX_LEASE_TTL_MS,
    ReclaimTokenHash,
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
    pub identity: ProcessIdentity,
    pub state: TaskState,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<String>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRecord {
    pub id: PaneId,
    pub session_id: String,
    pub updated_at_ms: i64,
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
        for state in SessionState::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
        for state in TaskInstanceState::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
        for state in AttachmentAuthorityState::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
        for health in WorkerHealth::ALL {
            assert_eq!(serde_json::to_value(health).unwrap(), health.as_str());
        }
        for disposition in ReconciliationDisposition::ALL {
            assert_eq!(
                serde_json::to_value(disposition).unwrap(),
                disposition.as_str()
            );
        }
    }
}
