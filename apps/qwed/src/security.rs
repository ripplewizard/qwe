use qwe_core::{
    AgentCommandRecord, CapabilityId, CapabilityOperation, CapabilityRecord, CapabilityScope,
    MAX_CAPABILITY_TTL_MS, PaneId, PrincipalEpoch, PrincipalId, PrincipalRecord, ReclaimTokenHash,
    SANDBOX_PROFILE_VERSION, SandboxAuditRecord, SandboxLandlockMode, SandboxProcessMode,
    SandboxProfile,
};
use qwe_protocol::{AuthContext, ProtocolErrorBody};
use qwe_runtime::resolve_beneath;
use qwe_storage::{StorageError, Store};
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SecurityGrant {
    pub capability: CapabilityRecord,
    pub principal: PrincipalRecord,
}

pub struct IssuedCapability {
    pub token: String,
    pub record: CapabilityRecord,
}

pub struct SandboxCapabilityGrant {
    pub profile: SandboxProfile,
    pub audit: SandboxAuditRecord,
}

pub fn initialize_bootstrap(
    store: &mut Store,
    runtime_dir: &Path,
    now_ms: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let principal = store.local_root_principal()?;
    if !principal.high_trust {
        return Err("local-root Principal is not high-trust".into());
    }
    let issued = issue_capability(
        store,
        &principal.id,
        CapabilityOperation::ManagePrincipals,
        CapabilityScope::Global,
        None,
        MAX_CAPABILITY_TTL_MS,
        now_ms,
    )
    .map_err(|failure| format!("{}: {}", failure.code, failure.message))?;
    write_bootstrap_token(&runtime_dir.join("security.bootstrap"), &issued.token)?;
    Ok(())
}

pub fn issue_capability(
    store: &mut Store,
    principal_id: &PrincipalId,
    operation: CapabilityOperation,
    scope: CapabilityScope,
    sandbox_grant: Option<SandboxCapabilityGrant>,
    ttl_ms: i64,
    now_ms: i64,
) -> Result<IssuedCapability, ProtocolErrorBody> {
    if ttl_ms <= 0 || ttl_ms > MAX_CAPABILITY_TTL_MS {
        return Err(error(
            "invalid_request",
            format!("ttl_ms must be between 1 and {MAX_CAPABILITY_TTL_MS}"),
        ));
    }
    validate_operation_scope(operation, &scope)?;
    validate_sandbox_grant(
        operation,
        sandbox_grant.as_ref().map(|grant| &grant.profile),
    )?;
    let chain = store.principal_chain(principal_id).map_err(storage_error)?;
    if chain
        .iter()
        .any(|principal| principal.revoked_at_ms.is_some())
    {
        return Err(error(
            "principal_revoked",
            "refusing to issue a capability to a revoked Principal",
        ));
    }
    let principal_epochs = chain
        .iter()
        .map(|principal| PrincipalEpoch {
            principal_id: principal.id.clone(),
            epoch: principal.epoch,
        })
        .collect::<Vec<_>>();
    let token = capability_token()?;
    let sandbox_profile = sandbox_grant.as_ref().map(|grant| grant.profile.clone());
    let sandbox_audit = sandbox_grant.map(|grant| grant.audit);
    let record = CapabilityRecord {
        id: CapabilityId::new(),
        token_hash: ReclaimTokenHash::from_token(&token),
        principal_id: principal_id.clone(),
        principal_epochs,
        operation,
        scope,
        created_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(ttl_ms),
        revoked_at_ms: None,
        sandbox_profile,
        sandbox_audit,
    };
    store.install_capability(&record).map_err(storage_error)?;
    Ok(IssuedCapability { token, record })
}

pub fn validate_sandbox_grant(
    operation: CapabilityOperation,
    sandbox_profile: Option<&SandboxProfile>,
) -> Result<(), ProtocolErrorBody> {
    let Some(profile) = sandbox_profile else {
        if operation == CapabilityOperation::SandboxedPane {
            return Err(error(
                "invalid_request",
                "sandboxed_pane capabilities require a sandbox_profile",
            ));
        }
        return Ok(());
    };
    if !matches!(
        operation,
        CapabilityOperation::AgentExec | CapabilityOperation::SandboxedPane
    ) {
        return Err(error(
            "invalid_request",
            "sandbox_profile is only valid for agent_exec or sandboxed_pane capabilities",
        ));
    }
    if profile.version != SANDBOX_PROFILE_VERSION {
        return Err(error(
            "sandbox_profile_unsupported",
            format!(
                "sandbox profile version {} is unsupported; expected {SANDBOX_PROFILE_VERSION}",
                profile.version
            ),
        ));
    }
    if !profile.seccomp.is_supported_v1() {
        return Err(error(
            "sandbox_profile_unsupported",
            "only the named seccomp profile 'strict' is implemented",
        ));
    }
    if matches!(
        profile.landlock,
        SandboxLandlockMode::Required { min_abi: 0 }
    ) {
        return Err(error(
            "sandbox_profile_unsupported",
            "Landlock min_abi must be greater than zero",
        ));
    }
    if !profile.workspace.is_absolute() {
        return Err(error(
            "invalid_request",
            "sandbox workspace must be an absolute path",
        ));
    }
    if !profile.workspace.is_dir() {
        return Err(error(
            "sandbox_profile_unsupported",
            format!(
                "sandbox workspace '{}' must be an existing directory",
                profile.workspace.display()
            ),
        ));
    }
    if profile
        .protected_paths
        .iter()
        .any(|path| !path.is_absolute())
    {
        return Err(error(
            "invalid_request",
            "sandbox protected paths must be absolute",
        ));
    }
    let mut resolved_protected = Vec::with_capacity(profile.protected_paths.len());
    for path in &profile.protected_paths {
        let resolved = resolve_beneath(&profile.workspace, path).map_err(|failure| {
            error(
                "sandbox_profile_unsupported",
                format!(
                    "sandbox protected path '{}' is unavailable or escapes the workspace: {failure}",
                    path.display()
                ),
            )
        })?;
        resolved_protected.push(resolved);
    }
    match qwe_core::find_hardlinked_protected_path(&resolved_protected) {
        Ok(None) => {}
        Ok(Some(path)) => {
            return Err(error(
                "sandbox_profile_unsupported",
                format!(
                    "protected path contains hardlinked file '{}'; hardlink aliases can bypass read-only protection",
                    path.display()
                ),
            ));
        }
        Err(failure) => {
            return Err(error(
                "sandbox_profile_unsupported",
                format!("failed to inspect protected paths for hardlinks: {failure}"),
            ));
        }
    }
    if !profile.private_tmp {
        return Err(error(
            "sandbox_profile_unsupported",
            "sandbox profile v1 requires a private /tmp",
        ));
    }
    if operation == CapabilityOperation::SandboxedPane
        && profile.process_namespace != SandboxProcessMode::Isolated
    {
        return Err(error(
            "sandbox_profile_unsupported",
            "sandboxed Pane v1 requires an isolated process namespace",
        ));
    }
    Ok(())
}

pub fn require_management(
    store: &Store,
    auth_context: Option<&AuthContext>,
    now_ms: i64,
) -> Result<SecurityGrant, ProtocolErrorBody> {
    authorize(store, auth_context, Authorization::Management, now_ms, None)
}

pub fn require_live_token(
    store: &Store,
    auth_context: Option<&AuthContext>,
    now_ms: i64,
) -> Result<SecurityGrant, ProtocolErrorBody> {
    authorize(store, auth_context, Authorization::Live, now_ms, None)
}

pub fn require_agent_exec(
    store: &Store,
    auth_context: Option<&AuthContext>,
    pane_id: &PaneId,
    now_ms: i64,
) -> Result<SecurityGrant, ProtocolErrorBody> {
    authorize(
        store,
        auth_context,
        Authorization::AgentExec(pane_id),
        now_ms,
        None,
    )
}

pub fn require_sandboxed_pane(
    store: &Store,
    auth_context: Option<&AuthContext>,
    session_id: &str,
    now_ms: i64,
) -> Result<SecurityGrant, ProtocolErrorBody> {
    let grant = authorize(
        store,
        auth_context,
        Authorization::SandboxedPane(session_id),
        now_ms,
        None,
    )?;
    if grant.capability.sandbox_profile.is_none() || grant.capability.sandbox_audit.is_none() {
        return Err(error(
            "capability_invalid",
            "sandboxed_pane capability has no enforced sandbox profile",
        ));
    }
    Ok(grant)
}

pub fn require_command_read(
    store: &Store,
    auth_context: Option<&AuthContext>,
    command: &AgentCommandRecord,
    now_ms: i64,
) -> Result<SecurityGrant, ProtocolErrorBody> {
    authorize(
        store,
        auth_context,
        Authorization::CommandRead,
        now_ms,
        Some(command),
    )
}

pub fn require_command_terminate(
    store: &Store,
    auth_context: Option<&AuthContext>,
    command: &AgentCommandRecord,
    now_ms: i64,
) -> Result<SecurityGrant, ProtocolErrorBody> {
    authorize(
        store,
        auth_context,
        Authorization::CommandTerminate,
        now_ms,
        Some(command),
    )
}

pub fn capability_json(issued: &IssuedCapability) -> serde_json::Value {
    json!({
        "capability_id": issued.record.id,
        "principal_id": issued.record.principal_id,
        "operation": issued.record.operation.as_str(),
        "scope": issued.record.scope,
        "created_at_ms": issued.record.created_at_ms,
        "expires_at_ms": issued.record.expires_at_ms,
        "sandbox_profile": issued.record.sandbox_profile,
        "sandbox_audit": issued.record.sandbox_audit,
        "token": issued.token
    })
}

#[derive(Debug, Clone, Copy)]
enum Authorization<'a> {
    Live,
    Management,
    AgentExec(&'a PaneId),
    SandboxedPane(&'a str),
    CommandRead,
    CommandTerminate,
}

fn authorize(
    store: &Store,
    auth_context: Option<&AuthContext>,
    authorization: Authorization<'_>,
    now_ms: i64,
    command: Option<&AgentCommandRecord>,
) -> Result<SecurityGrant, ProtocolErrorBody> {
    let token = auth_context
        .map(|context| context.capability.trim())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            error(
                "capability_missing",
                "this operation requires an AuthContext capability",
            )
        })?;
    let active = store
        .find_capability_by_hash(&ReclaimTokenHash::from_token(token))
        .map_err(storage_error)?
        .ok_or_else(|| error("capability_invalid", "capability token is invalid"))?;
    if active.revoked_at_ms.is_some() {
        return Err(error("capability_revoked", "capability has been revoked"));
    }
    if active.expires_at_ms <= now_ms {
        return Err(error("capability_expired", "capability has expired"));
    }
    let chain = store
        .principal_chain(&active.principal_id)
        .map_err(storage_error)?;
    if chain
        .iter()
        .any(|principal| principal.revoked_at_ms.is_some())
    {
        return Err(error(
            "principal_revoked",
            "capability Principal or an ancestor has been revoked",
        ));
    }
    let current_epochs = chain
        .iter()
        .map(|principal| PrincipalEpoch {
            principal_id: principal.id.clone(),
            epoch: principal.epoch,
        })
        .collect::<Vec<_>>();
    if current_epochs != active.principal_epochs {
        return Err(error(
            "capability_epoch_stale",
            "capability Principal epoch chain has changed",
        ));
    }
    let principal = chain
        .first()
        .cloned()
        .ok_or_else(|| error("capability_invalid", "capability Principal is missing"))?;

    let allowed = match authorization {
        Authorization::Live => true,
        Authorization::Management => {
            active.operation == CapabilityOperation::ManagePrincipals
                && active.scope == CapabilityScope::Global
        }
        Authorization::AgentExec(pane_id) => {
            active.operation == CapabilityOperation::AgentExec
                && active.scope
                    == (CapabilityScope::Pane {
                        pane_id: pane_id.clone(),
                    })
        }
        Authorization::SandboxedPane(session_id) => {
            active.operation == CapabilityOperation::SandboxedPane
                && active.scope
                    == (CapabilityScope::Session {
                        session_id: session_id.to_string(),
                    })
        }
        Authorization::CommandRead => command.is_some_and(|command| {
            command_scope_allowed(&active, command, CapabilityOperation::AgentCommandRead)
        }),
        Authorization::CommandTerminate => command.is_some_and(|command| {
            command_scope_allowed(&active, command, CapabilityOperation::AgentCommandTerminate)
        }),
    };
    if !allowed {
        return Err(error(
            "capability_scope_denied",
            "capability does not authorize this operation and target",
        ));
    }
    Ok(SecurityGrant {
        capability: active,
        principal,
    })
}

fn command_scope_allowed(
    capability: &CapabilityRecord,
    command: &AgentCommandRecord,
    operation: CapabilityOperation,
) -> bool {
    if command.creator_capability_id.as_ref() == Some(&capability.id)
        && capability.operation == CapabilityOperation::AgentExec
        && capability.scope
            == (CapabilityScope::Pane {
                pane_id: command.pane_id.clone(),
            })
    {
        return true;
    }
    capability.operation == operation
        && capability.scope
            == (CapabilityScope::Command {
                pane_id: command.pane_id.clone(),
                command_id: command.id.clone(),
            })
}

fn validate_operation_scope(
    operation: CapabilityOperation,
    scope: &CapabilityScope,
) -> Result<(), ProtocolErrorBody> {
    let valid = match operation {
        CapabilityOperation::ManagePrincipals => matches!(scope, CapabilityScope::Global),
        CapabilityOperation::AgentExec => matches!(scope, CapabilityScope::Pane { .. }),
        CapabilityOperation::AgentCommandRead | CapabilityOperation::AgentCommandTerminate => {
            matches!(scope, CapabilityScope::Command { .. })
        }
        CapabilityOperation::SandboxedPane => matches!(scope, CapabilityScope::Session { .. }),
        CapabilityOperation::PaneExec
        | CapabilityOperation::PaneInput
        | CapabilityOperation::PaneResize => matches!(scope, CapabilityScope::Pane { .. }),
    };
    if valid {
        Ok(())
    } else {
        Err(error(
            "invalid_request",
            "capability operation and scope do not match",
        ))
    }
}

fn capability_token() -> Result<String, ProtocolErrorBody> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|failure| error("internal", format!("random source failed: {failure}")))?;
    Ok(hex_encode(&bytes))
}

fn write_bootstrap_token(path: &Path, token: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()
}

fn storage_error(failure: StorageError) -> ProtocolErrorBody {
    error("persistence_failure", failure.to_string())
}

fn error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
        details: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use qwe_core::{SandboxLandlockMode, SandboxSeccompMode};
    use std::path::PathBuf;

    fn profile() -> SandboxProfile {
        SandboxProfile::new(std::env::temp_dir())
    }

    #[test]
    fn sandbox_profile_is_limited_to_sandbox_capabilities() {
        let failure =
            validate_sandbox_grant(CapabilityOperation::AgentCommandRead, Some(&profile()))
                .unwrap_err();
        assert_eq!(failure.code, "invalid_request");

        validate_sandbox_grant(CapabilityOperation::SandboxedPane, Some(&profile())).unwrap();
        let failure = validate_sandbox_grant(CapabilityOperation::SandboxedPane, None).unwrap_err();
        assert_eq!(failure.code, "invalid_request");
    }

    #[test]
    fn sandbox_workspace_and_protected_paths_must_be_absolute() {
        let mut relative_workspace = profile();
        relative_workspace.workspace = PathBuf::from("relative");
        let failure =
            validate_sandbox_grant(CapabilityOperation::AgentExec, Some(&relative_workspace))
                .unwrap_err();
        assert_eq!(failure.code, "invalid_request");

        let mut relative_protected = profile();
        relative_protected
            .protected_paths
            .push(PathBuf::from(".git/hooks"));
        let failure =
            validate_sandbox_grant(CapabilityOperation::AgentExec, Some(&relative_protected))
                .unwrap_err();
        assert_eq!(failure.code, "invalid_request");
    }

    #[test]
    fn sandbox_profile_fields_are_versioned_and_strict_seccomp_is_supported() {
        let mut wrong_version = profile();
        wrong_version.version = 2;
        let failure = validate_sandbox_grant(CapabilityOperation::AgentExec, Some(&wrong_version))
            .unwrap_err();
        assert_eq!(failure.code, "sandbox_profile_unsupported");

        let mut named_seccomp = profile();
        named_seccomp.seccomp = SandboxSeccompMode::Named {
            profile: "strict".into(),
        };
        validate_sandbox_grant(CapabilityOperation::AgentExec, Some(&named_seccomp)).unwrap();
        named_seccomp.seccomp = SandboxSeccompMode::Named {
            profile: "unknown".into(),
        };
        let failure = validate_sandbox_grant(CapabilityOperation::AgentExec, Some(&named_seccomp))
            .unwrap_err();
        assert_eq!(failure.code, "sandbox_profile_unsupported");
    }

    #[test]
    fn landlock_required_min_abi_must_be_positive() {
        let mut profile = profile();
        profile.landlock = SandboxLandlockMode::Required { min_abi: 0 };
        let failure =
            validate_sandbox_grant(CapabilityOperation::AgentExec, Some(&profile)).unwrap_err();
        assert_eq!(failure.code, "sandbox_profile_unsupported");
        profile.landlock = SandboxLandlockMode::Required { min_abi: 1 };
        validate_sandbox_grant(CapabilityOperation::AgentExec, Some(&profile)).unwrap();
    }

    #[test]
    fn pane_capabilities_require_their_intended_scope() {
        use qwe_core::CapabilityScope;
        let pane_id = PaneId::new();
        assert!(
            validate_operation_scope(
                CapabilityOperation::SandboxedPane,
                &CapabilityScope::Session {
                    session_id: "session".into()
                }
            )
            .is_ok()
        );
        for operation in [
            CapabilityOperation::PaneExec,
            CapabilityOperation::PaneInput,
            CapabilityOperation::PaneResize,
        ] {
            assert!(
                validate_operation_scope(
                    operation,
                    &CapabilityScope::Pane {
                        pane_id: pane_id.clone()
                    }
                )
                .is_ok()
            );
            assert!(
                validate_operation_scope(
                    operation,
                    &CapabilityScope::Session {
                        session_id: "session".into()
                    }
                )
                .is_err()
            );
        }
    }

    #[test]
    fn absent_sandbox_profile_retains_unsandboxed_compatibility() {
        validate_sandbox_grant(CapabilityOperation::AgentExec, None).unwrap();
    }
}
