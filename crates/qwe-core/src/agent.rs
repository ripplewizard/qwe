use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

use crate::{CapabilityId, PaneId, PrincipalId, ProcessIdentity};

pub const AGENT_OUTPUT_CHUNK_BYTES: usize = 64 * 1024;
pub const AGENT_REPLAY_LIMIT_BYTES: usize = 1024 * 1024;
pub const AGENT_MAX_WAIT_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSignal {
    Sigint,
    Sigterm,
    Sighup,
    Sigquit,
}

impl AgentSignal {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sigint => "SIGINT",
            Self::Sigterm => "SIGTERM",
            Self::Sighup => "SIGHUP",
            Self::Sigquit => "SIGQUIT",
        }
    }
}

impl fmt::Display for AgentSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for AgentSignal {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "SIGINT" => Ok(Self::Sigint),
            "SIGTERM" => Ok(Self::Sigterm),
            "SIGHUP" => Ok(Self::Sighup),
            "SIGQUIT" => Ok(Self::Sigquit),
            _ => Err(format!(
                "signal '{value}' is not allowed; use SIGINT, SIGTERM, SIGHUP, or SIGQUIT"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentCommandId(Uuid);

impl AgentCommandId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for AgentCommandId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AgentCommandId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for AgentCommandId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandState {
    Starting,
    Running,
    Idle,
    Done,
    Failed,
    Lost,
}

impl AgentCommandState {
    pub const ALL: [Self; 6] = [
        Self::Starting,
        Self::Running,
        Self::Idle,
        Self::Done,
        Self::Failed,
        Self::Lost,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Lost => "lost",
        }
    }

    #[must_use]
    pub fn from_db(value: &str) -> Self {
        match value {
            "starting" => Self::Starting,
            "running" => Self::Running,
            "idle" => Self::Idle,
            "done" => Self::Done,
            "failed" => Self::Failed,
            _ => Self::Lost,
        }
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Lost)
    }

    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Starting, Self::Running | Self::Failed | Self::Lost)
                | (
                    Self::Running,
                    Self::Idle | Self::Done | Self::Failed | Self::Lost
                )
                | (
                    Self::Idle,
                    Self::Running | Self::Done | Self::Failed | Self::Lost
                )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWaitState {
    Idle,
    Done,
    Failed,
    Lost,
    Unknown,
}

impl AgentWaitState {
    #[must_use]
    pub fn from_command_state(state: AgentCommandState) -> Option<Self> {
        match state {
            AgentCommandState::Idle => Some(Self::Idle),
            AgentCommandState::Done => Some(Self::Done),
            AgentCommandState::Failed => Some(Self::Failed),
            AgentCommandState::Lost => Some(Self::Lost),
            AgentCommandState::Starting | AgentCommandState::Running => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutputMode {
    PtyMerged,
    PipeSplit,
}

impl AgentOutputMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PtyMerged => "pty_merged",
            Self::PipeSplit => "pipe_split",
        }
    }

    #[must_use]
    pub fn from_db(value: &str) -> Self {
        if value == "pty_merged" {
            Self::PtyMerged
        } else {
            Self::PipeSplit
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutputStream {
    Stdout,
    Stderr,
    Pty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCommandRecord {
    pub id: AgentCommandId,
    pub session_id: String,
    pub pane_id: PaneId,
    pub owner: String,
    pub principal_id: PrincipalId,
    pub creator_capability_id: Option<CapabilityId>,
    pub worker_identity: ProcessIdentity,
    pub process_identity: Option<ProcessIdentity>,
    pub state: AgentCommandState,
    pub output_mode: AgentOutputMode,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub sequence: u64,
    pub command_id: AgentCommandId,
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub kind: AgentEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEventKind {
    CommandAccepted {
        state: AgentCommandState,
    },
    ProcessStarted {
        process_identity: ProcessIdentity,
    },
    StateChanged {
        previous: AgentCommandState,
        state: AgentCommandState,
    },
    Output {
        stream: AgentOutputStream,
        cursor: u64,
        data_hex: String,
    },
    IdleObserved,
    TerminalResult {
        exit_code: Option<i32>,
        exit_signal: Option<String>,
        error: Option<String>,
    },
    CommandLost {
        reason: String,
    },
    RedactionApplied {
        policy: String,
    },
    RevocationApplied {
        severity: String,
        deadline_ms: Option<i64>,
        reason: String,
    },
    SignalDelivered {
        signal: String,
    },
    SandboxRiskObserved {
        network_inherit: bool,
        writable_workspace: bool,
        credential_exposed: bool,
    },
    RetentionGap {
        from_sequence: u64,
        to_sequence: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_never_resurrect() {
        for terminal in [
            AgentCommandState::Done,
            AgentCommandState::Failed,
            AgentCommandState::Lost,
        ] {
            assert!(terminal.is_terminal());
            for next in AgentCommandState::ALL {
                assert!(!terminal.can_transition_to(next));
            }
        }
    }

    #[test]
    fn done_takes_precedence_over_idle_wait_result() {
        assert_eq!(
            AgentWaitState::from_command_state(AgentCommandState::Done),
            Some(AgentWaitState::Done)
        );
        assert_eq!(
            AgentWaitState::from_command_state(AgentCommandState::Idle),
            Some(AgentWaitState::Idle)
        );
    }

    #[test]
    fn command_id_round_trips_as_text() {
        let id = AgentCommandId::new();
        assert_eq!(id.to_string().parse::<AgentCommandId>(), Ok(id));
    }

    #[test]
    fn agent_signal_whitelist_is_strict() {
        assert_eq!("SIGINT".parse::<AgentSignal>(), Ok(AgentSignal::Sigint));
        assert_eq!("SIGTERM".parse::<AgentSignal>(), Ok(AgentSignal::Sigterm));
        assert_eq!("SIGHUP".parse::<AgentSignal>(), Ok(AgentSignal::Sighup));
        assert_eq!("SIGQUIT".parse::<AgentSignal>(), Ok(AgentSignal::Sigquit));
        assert!("SIGKILL".parse::<AgentSignal>().is_err());
        assert!("9".parse::<AgentSignal>().is_err());
    }
}
