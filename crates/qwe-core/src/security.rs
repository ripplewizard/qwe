use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

use crate::{AgentCommandId, PaneId, PrincipalId, ReclaimTokenHash, SandboxProfile};

pub const DEFAULT_CAPABILITY_TTL_MS: i64 = 24 * 60 * 60 * 1_000;
pub const MAX_CAPABILITY_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
pub const DEFAULT_FREEZE_GRACE_MS: i64 = 60_000;
pub const MAX_FREEZE_GRACE_MS: i64 = 5 * 60_000;
pub const MAX_PANE_INPUT_AUDIT_ROWS: u64 = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CapabilityId(Uuid);

impl CapabilityId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for CapabilityId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for CapabilityId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOperation {
    ManagePrincipals,
    AgentExec,
    AgentCommandRead,
    AgentCommandTerminate,
    SandboxedPane,
    PaneExec,
    PaneInput,
    PaneResize,
}

impl CapabilityOperation {
    pub const ALL: [Self; 8] = [
        Self::ManagePrincipals,
        Self::AgentExec,
        Self::AgentCommandRead,
        Self::AgentCommandTerminate,
        Self::SandboxedPane,
        Self::PaneExec,
        Self::PaneInput,
        Self::PaneResize,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManagePrincipals => "manage_principals",
            Self::AgentExec => "agent_exec",
            Self::AgentCommandRead => "agent_command_read",
            Self::AgentCommandTerminate => "agent_command_terminate",
            Self::SandboxedPane => "sandboxed_pane",
            Self::PaneExec => "pane_exec",
            Self::PaneInput => "pane_input",
            Self::PaneResize => "pane_resize",
        }
    }

    #[must_use]
    pub fn from_db(value: &str) -> Self {
        match value {
            "manage_principals" => Self::ManagePrincipals,
            "agent_command_read" => Self::AgentCommandRead,
            "agent_command_terminate" => Self::AgentCommandTerminate,
            "sandboxed_pane" => Self::SandboxedPane,
            "pane_exec" => Self::PaneExec,
            "pane_input" => Self::PaneInput,
            "pane_resize" => Self::PaneResize,
            _ => Self::AgentExec,
        }
    }
}

impl FromStr for CapabilityOperation {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "manage_principals" => Ok(Self::ManagePrincipals),
            "agent_exec" => Ok(Self::AgentExec),
            "agent_command_read" => Ok(Self::AgentCommandRead),
            "agent_command_terminate" => Ok(Self::AgentCommandTerminate),
            "sandboxed_pane" => Ok(Self::SandboxedPane),
            "pane_exec" => Ok(Self::PaneExec),
            "pane_input" => Ok(Self::PaneInput),
            "pane_resize" => Ok(Self::PaneResize),
            _ => Err(format!("unknown capability operation '{value}'")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CapabilityScope {
    Global,
    Session {
        session_id: String,
    },
    Pane {
        pane_id: PaneId,
    },
    Command {
        pane_id: PaneId,
        command_id: AgentCommandId,
    },
}

impl CapabilityScope {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Session { .. } => "session",
            Self::Pane { .. } => "pane",
            Self::Command { .. } => "command",
        }
    }

    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Session { session_id } => Some(session_id),
            _ => None,
        }
    }

    #[must_use]
    pub fn pane_id(&self) -> Option<&PaneId> {
        match self {
            Self::Pane { pane_id } | Self::Command { pane_id, .. } => Some(pane_id),
            _ => None,
        }
    }

    #[must_use]
    pub fn command_id(&self) -> Option<&AgentCommandId> {
        match self {
            Self::Command { command_id, .. } => Some(command_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationSeverity {
    Administrative,
    #[default]
    Freeze,
    Terminate,
}

impl RevocationSeverity {
    pub const ALL: [Self; 3] = [Self::Administrative, Self::Freeze, Self::Terminate];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Administrative => "administrative",
            Self::Freeze => "freeze",
            Self::Terminate => "terminate",
        }
    }

    #[must_use]
    pub fn from_db(value: &str) -> Self {
        match value {
            "administrative" => Self::Administrative,
            "terminate" => Self::Terminate,
            _ => Self::Freeze,
        }
    }
}

impl FromStr for RevocationSeverity {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "administrative" | "admin" => Ok(Self::Administrative),
            "freeze" => Ok(Self::Freeze),
            "terminate" => Ok(Self::Terminate),
            _ => Err(format!("unknown revocation severity '{value}'")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalEpoch {
    pub principal_id: PrincipalId,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxAuditRecord {
    pub profile_hash: String,
    pub backend_path: String,
    pub backend_version: String,
    pub probe_time_ms: i64,
    #[serde(default)]
    pub seccomp_profile_hash: Option<String>,
    #[serde(default)]
    pub landlock_abi: Option<u32>,
    #[serde(default)]
    pub fd_bind: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInputAuditRecord {
    pub sequence: u64,
    pub pane_id: crate::PaneId,
    pub principal_id: PrincipalId,
    pub capability_id: CapabilityId,
    pub recorded_at_ms: i64,
    pub byte_length: u64,
    pub maximum_bytes: u64,
    pub profile_version: u16,
    pub landlock_abi: Option<u32>,
    pub content_recorded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRecord {
    pub id: CapabilityId,
    pub token_hash: ReclaimTokenHash,
    pub principal_id: PrincipalId,
    pub principal_epochs: Vec<PrincipalEpoch>,
    pub operation: CapabilityOperation,
    pub scope: CapabilityScope,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
    pub sandbox_profile: Option<SandboxProfile>,
    pub sandbox_audit: Option<SandboxAuditRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unspecified_revocation_fails_safe_to_freeze() {
        assert_eq!(
            serde_json::from_str::<RevocationSeverity>("null").unwrap_or_default(),
            RevocationSeverity::Freeze
        );
        assert_eq!(RevocationSeverity::default(), RevocationSeverity::Freeze);
    }

    #[test]
    fn capability_operation_round_trips_as_text() {
        for operation in CapabilityOperation::ALL {
            assert_eq!(
                operation.as_str().parse::<CapabilityOperation>(),
                Ok(operation)
            );
        }
    }
}
