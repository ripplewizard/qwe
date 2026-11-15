use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub const SANDBOX_PROFILE_VERSION: u16 = 1;
pub const SANDBOX_SECCOMP_STRICT_PROFILE: &str = "strict";

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
}
