use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const SANDBOX_PROFILE_VERSION: u16 = 1;
pub const SANDBOX_SECCOMP_STRICT_PROFILE: &str = "strict";
pub const SANDBOXED_PANE_ORPHAN_TTL_MS: i64 = 15 * 60 * 1_000;
pub const MAX_ORPHANED_SANDBOXED_PANES: usize = 16;
pub const SANDBOXED_PANE_SHUTDOWN_GRACE_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxWorkspaceMode {
    #[default]
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxNetworkMode {
    #[default]
    IsolatedLoopback,
    Inherit,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxHomeMode {
    #[default]
    Hidden,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxProcessMode {
    #[default]
    Isolated,
    Shared,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SandboxSeccompMode {
    #[default]
    None,
    Named {
        profile: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SandboxLandlockMode {
    Off,
    #[default]
    BestEffort,
    Required {
        min_abi: u32,
    },
}

impl SandboxSeccompMode {
    #[must_use]
    pub fn is_supported_v1(&self) -> bool {
        match self {
            Self::None => true,
            Self::Named { profile } => profile == SANDBOX_SECCOMP_STRICT_PROFILE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxProfile {
    pub version: u16,
    pub workspace: PathBuf,
    #[serde(default)]
    pub workspace_mode: SandboxWorkspaceMode,
    #[serde(default)]
    pub protected_paths: Vec<PathBuf>,
    #[serde(default)]
    pub network: SandboxNetworkMode,
    #[serde(default)]
    pub home: SandboxHomeMode,
    #[serde(default = "default_true")]
    pub private_tmp: bool,
    #[serde(default)]
    pub process_namespace: SandboxProcessMode,
    #[serde(default)]
    pub seccomp: SandboxSeccompMode,
    #[serde(default)]
    pub landlock: SandboxLandlockMode,
}

impl SandboxProfile {
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            version: SANDBOX_PROFILE_VERSION,
            workspace,
            workspace_mode: SandboxWorkspaceMode::ReadOnly,
            protected_paths: Vec::new(),
            network: SandboxNetworkMode::IsolatedLoopback,
            home: SandboxHomeMode::Hidden,
            private_tmp: true,
            process_namespace: SandboxProcessMode::Isolated,
            seccomp: SandboxSeccompMode::None,
            landlock: SandboxLandlockMode::BestEffort,
        }
    }

    #[must_use]
    pub fn is_enforced_v1(&self) -> bool {
        self.version == SANDBOX_PROFILE_VERSION
            && self.seccomp.is_supported_v1()
            && match self.landlock {
                SandboxLandlockMode::Required { min_abi } => min_abi > 0,
                SandboxLandlockMode::Off | SandboxLandlockMode::BestEffort => true,
            }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SandboxRefinementError {
    #[error("sandbox profile version cannot change during refinement")]
    VersionChanged,
    #[error("sandbox workspace cannot change during refinement")]
    WorkspaceChanged,
    #[error("sandbox protected-path set cannot change during refinement")]
    ProtectedPathsChanged,
    #[error("sandbox home mode is fixed during refinement")]
    HomeChanged,
    #[error("sandbox private-tmp policy is fixed during refinement")]
    PrivateTmpChanged,
    #[error("sandbox process namespace is fixed during refinement")]
    ProcessNamespaceChanged,
    #[error("sandbox workspace mode would broaden")]
    WorkspaceBroadened,
    #[error("sandbox network mode would broaden")]
    NetworkBroadened,
    #[error("sandbox seccomp mode would broaden")]
    SeccompBroadened,
    #[error("sandbox Landlock mode would broaden")]
    LandlockBroadened,
}

#[derive(Debug, Clone, Copy)]
pub struct RefinedSandboxProfile<'a> {
    profile: &'a SandboxProfile,
}

impl<'a> RefinedSandboxProfile<'a> {
    pub fn new(
        parent: &SandboxProfile,
        child: &'a SandboxProfile,
    ) -> Result<Self, SandboxRefinementError> {
        child.validate_refinement_of(parent)?;
        Ok(Self { profile: child })
    }

    #[must_use]
    pub fn profile(&self) -> &'a SandboxProfile {
        self.profile
    }
}

impl SandboxProfile {
    pub fn validate_refinement_of(
        &self,
        parent: &SandboxProfile,
    ) -> Result<(), SandboxRefinementError> {
        if self.version != parent.version {
            return Err(SandboxRefinementError::VersionChanged);
        }
        if self.workspace != parent.workspace {
            return Err(SandboxRefinementError::WorkspaceChanged);
        }
        let mut child_paths = self.protected_paths.clone();
        let mut parent_paths = parent.protected_paths.clone();
        child_paths.sort();
        parent_paths.sort();
        if child_paths != parent_paths {
            return Err(SandboxRefinementError::ProtectedPathsChanged);
        }
        if self.home != parent.home {
            return Err(SandboxRefinementError::HomeChanged);
        }
        if self.private_tmp != parent.private_tmp {
            return Err(SandboxRefinementError::PrivateTmpChanged);
        }
        if self.process_namespace != parent.process_namespace {
            return Err(SandboxRefinementError::ProcessNamespaceChanged);
        }
        if parent.workspace_mode == SandboxWorkspaceMode::ReadOnly
            && self.workspace_mode != SandboxWorkspaceMode::ReadOnly
        {
            return Err(SandboxRefinementError::WorkspaceBroadened);
        }
        if parent.network == SandboxNetworkMode::IsolatedLoopback
            && self.network != SandboxNetworkMode::IsolatedLoopback
        {
            return Err(SandboxRefinementError::NetworkBroadened);
        }
        if matches!(parent.seccomp, SandboxSeccompMode::Named { .. })
            && self.seccomp != parent.seccomp
        {
            return Err(SandboxRefinementError::SeccompBroadened);
        }
        if !landlock_refines(self.landlock, parent.landlock) {
            return Err(SandboxRefinementError::LandlockBroadened);
        }
        Ok(())
    }
}

fn landlock_refines(child: SandboxLandlockMode, parent: SandboxLandlockMode) -> bool {
    match (parent, child) {
        (SandboxLandlockMode::Off, _) => true,
        (SandboxLandlockMode::BestEffort, SandboxLandlockMode::BestEffort)
        | (SandboxLandlockMode::BestEffort, SandboxLandlockMode::Required { .. }) => true,
        (
            SandboxLandlockMode::Required {
                min_abi: parent_min,
            },
            SandboxLandlockMode::Required { min_abi: child_min },
        ) => child_min >= parent_min && child_min > 0,
        _ => false,
    }
}

pub fn find_hardlinked_protected_path(protected_paths: &[PathBuf]) -> io::Result<Option<PathBuf>> {
    for path in protected_paths {
        let target = fs::canonicalize(path)?;
        if let Some(path) = find_hardlink(&target)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn find_hardlink(path: &Path) -> io::Result<Option<PathBuf>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            if let Some(path) = find_hardlink(&entry?.path())? {
                return Ok(Some(path));
            }
        }
        Ok(None)
    } else if metadata.nlink() > 1 {
        Ok(Some(path.to_path_buf()))
    } else {
        Ok(None)
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_defaults_fail_closed() {
        let profile: SandboxProfile = serde_json::from_value(serde_json::json!({
            "version": 1,
            "workspace": "/tmp/workspace"
        }))
        .unwrap();
        assert_eq!(profile.workspace_mode, SandboxWorkspaceMode::ReadOnly);
        assert_eq!(profile.network, SandboxNetworkMode::IsolatedLoopback);
        assert_eq!(profile.home, SandboxHomeMode::Hidden);
        assert!(profile.private_tmp);
        assert_eq!(profile.process_namespace, SandboxProcessMode::Isolated);
        assert_eq!(profile.seccomp, SandboxSeccompMode::None);
        assert_eq!(profile.landlock, SandboxLandlockMode::BestEffort);
        assert!(profile.is_enforced_v1());
    }

    #[test]
    fn unknown_profile_fields_are_rejected() {
        let result = serde_json::from_value::<SandboxProfile>(serde_json::json!({
            "version": 1,
            "workspace": "/tmp/workspace",
            "network": "inherit",
            "netwrok": "isolated_loopback"
        }));
        assert!(result.is_err());
    }

    #[test]
    fn named_seccomp_only_accepts_strict() {
        let mut profile = SandboxProfile::new(PathBuf::from("/tmp/workspace"));
        profile.seccomp = SandboxSeccompMode::Named {
            profile: "strict".into(),
        };
        assert!(profile.is_enforced_v1());
        profile.seccomp = SandboxSeccompMode::Named {
            profile: "unknown".into(),
        };
        assert!(!profile.is_enforced_v1());
    }

    #[test]
    fn landlock_mode_is_versioned_and_min_abi_must_be_positive() {
        let mut profile = SandboxProfile::new(PathBuf::from("/tmp/workspace"));
        profile.landlock = SandboxLandlockMode::Off;
        assert!(profile.is_enforced_v1());
        profile.landlock = SandboxLandlockMode::Required { min_abi: 1 };
        assert!(profile.is_enforced_v1());
        profile.landlock = SandboxLandlockMode::Required { min_abi: 0 };
        assert!(!profile.is_enforced_v1());

        let serialized = serde_json::to_value(SandboxProfile {
            landlock: SandboxLandlockMode::Required { min_abi: 3 },
            ..SandboxProfile::new(PathBuf::from("/tmp/workspace"))
        })
        .unwrap();
        assert_eq!(
            serialized["landlock"],
            serde_json::json!({"mode": "required", "min_abi": 3})
        );
    }

    #[test]
    fn sandboxed_pane_resource_defaults_are_stable() {
        assert_eq!(SANDBOXED_PANE_ORPHAN_TTL_MS, 900_000);
        assert_eq!(MAX_ORPHANED_SANDBOXED_PANES, 16);
        assert_eq!(SANDBOXED_PANE_SHUTDOWN_GRACE_MS, 5_000);
    }

    #[test]
    fn profile_refinement_can_only_tighten_stackable_fields() {
        let mut parent = SandboxProfile::new(PathBuf::from("/tmp/workspace"));
        parent.workspace_mode = SandboxWorkspaceMode::ReadWrite;
        parent.network = SandboxNetworkMode::Inherit;
        parent.seccomp = SandboxSeccompMode::None;
        parent.landlock = SandboxLandlockMode::Off;

        let mut child = parent.clone();
        child.workspace_mode = SandboxWorkspaceMode::ReadOnly;
        child.network = SandboxNetworkMode::IsolatedLoopback;
        child.seccomp = SandboxSeccompMode::Named {
            profile: SANDBOX_SECCOMP_STRICT_PROFILE.into(),
        };
        child.landlock = SandboxLandlockMode::Required { min_abi: 1 };
        assert_eq!(
            RefinedSandboxProfile::new(&parent, &child)
                .unwrap()
                .profile(),
            &child
        );

        let mut isolated_parent = parent.clone();
        isolated_parent.network = SandboxNetworkMode::IsolatedLoopback;
        let mut broadened_network = isolated_parent.clone();
        broadened_network.network = SandboxNetworkMode::Inherit;
        assert_eq!(
            broadened_network
                .validate_refinement_of(&isolated_parent)
                .unwrap_err(),
            SandboxRefinementError::NetworkBroadened
        );
    }

    #[test]
    fn profile_refinement_rejects_fixed_fields_and_broadened_modes() {
        let parent = SandboxProfile::new(PathBuf::from("/tmp/workspace"));
        let mut changed_workspace = parent.clone();
        changed_workspace.workspace = PathBuf::from("/tmp/other");
        assert_eq!(
            changed_workspace
                .validate_refinement_of(&parent)
                .unwrap_err(),
            SandboxRefinementError::WorkspaceChanged
        );

        let mut changed_tmp = parent.clone();
        changed_tmp.private_tmp = false;
        assert_eq!(
            changed_tmp.validate_refinement_of(&parent).unwrap_err(),
            SandboxRefinementError::PrivateTmpChanged
        );

        let mut broadened_seccomp = parent.clone();
        broadened_seccomp.seccomp = SandboxSeccompMode::None;
        let mut strict_parent = parent.clone();
        strict_parent.seccomp = SandboxSeccompMode::Named {
            profile: SANDBOX_SECCOMP_STRICT_PROFILE.into(),
        };
        assert_eq!(
            broadened_seccomp
                .validate_refinement_of(&strict_parent)
                .unwrap_err(),
            SandboxRefinementError::SeccompBroadened
        );

        let mut required_parent = parent.clone();
        required_parent.landlock = SandboxLandlockMode::Required { min_abi: 3 };
        let mut lower_landlock = parent.clone();
        lower_landlock.landlock = SandboxLandlockMode::Required { min_abi: 2 };
        assert_eq!(
            lower_landlock
                .validate_refinement_of(&required_parent)
                .unwrap_err(),
            SandboxRefinementError::LandlockBroadened
        );
    }
}
