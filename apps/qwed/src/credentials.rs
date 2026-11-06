use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use thiserror::Error;
use zeroize::Zeroizing;

const MAX_CREDENTIAL_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialBackend {
    Filesystem,
    Keychain,
    None,
}

impl CredentialBackend {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem",
            Self::Keychain => "keychain",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("credential provider is not configured")]
    NotConfigured,
    #[error("invalid credential name")]
    InvalidName,
    #[error("credential '{0}' was not found")]
    NotFound(String),
    #[error("credential '{0}' must be a regular file, not a symlink")]
    InvalidFile(String),
    #[error("credential '{name}' is too large ({bytes} bytes; maximum {maximum})")]
    TooLarge {
        name: String,
        bytes: u64,
        maximum: u64,
    },
    #[error("credential directory or file permissions are too broad")]
    InsecurePermissions,
    #[error("keychain lookup failed for credential '{0}'")]
    Keychain(String),
    #[error("credential I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

pub struct CredentialProvider {
    root: Option<PathBuf>,
    keychain_prefix: Option<String>,
}

impl CredentialProvider {
    #[must_use]
    pub fn new(root: Option<PathBuf>, keychain_prefix: Option<String>) -> Self {
        Self {
            root,
            keychain_prefix,
        }
    }

    #[must_use]
    pub fn backend(&self) -> CredentialBackend {
        if self.root.is_some() {
            CredentialBackend::Filesystem
        } else if self.keychain_prefix.is_some() {
            CredentialBackend::Keychain
        } else {
            CredentialBackend::None
        }
    }

    pub fn names(&self) -> Result<Vec<String>, CredentialError> {
        let Some(root) = &self.root else {
            return Ok(Vec::new());
        };
        validate_private_directory(root)?;
        let mut names = Vec::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if validate_name(&name).is_ok() && entry.file_type()?.is_file() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn read(&self, name: &str) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        validate_name(name)?;
        if let Some(root) = &self.root {
            return read_file(root, name);
        }
        if let Some(prefix) = &self.keychain_prefix {
            return read_keychain(prefix, name);
        }
        Err(CredentialError::NotConfigured)
    }
}

fn read_file(root: &Path, name: &str) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
    validate_private_directory(root)?;
    let path = root.join(name);
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|failure| {
            if failure.kind() == std::io::ErrorKind::NotFound {
                CredentialError::NotFound(name.into())
            } else if failure.raw_os_error() == Some(libc::ELOOP) {
                CredentialError::InvalidFile(name.into())
            } else {
                CredentialError::Io(failure)
            }
        })?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(CredentialError::InvalidFile(name.into()));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(CredentialError::InsecurePermissions);
    }
    if metadata.len() > MAX_CREDENTIAL_BYTES {
        return Err(CredentialError::TooLarge {
            name: name.into(),
            bytes: metadata.len(),
            maximum: MAX_CREDENTIAL_BYTES,
        });
    }
    let mut secret = Zeroizing::new(Vec::with_capacity(metadata.len() as usize));
    file.take(MAX_CREDENTIAL_BYTES + 1)
        .read_to_end(&mut secret)?;
    if secret.len() as u64 > MAX_CREDENTIAL_BYTES {
        return Err(CredentialError::TooLarge {
            name: name.into(),
            bytes: secret.len() as u64,
            maximum: MAX_CREDENTIAL_BYTES,
        });
    }
    Ok(secret)
}

fn validate_private_directory(root: &Path) -> Result<(), CredentialError> {
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(CredentialError::InsecurePermissions);
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), CredentialError> {
    if name.is_empty()
        || name.len() > 128
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(CredentialError::InvalidName);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn read_keychain(prefix: &str, name: &str) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
    let service = format!("{prefix}.{name}");
    let output = Command::new("/usr/bin/security")
        .arg("find-generic-password")
        .arg("-s")
        .arg(&service)
        .arg("-w")
        .output()
        .map_err(|_| CredentialError::Keychain(name.into()))?;
    if !output.status.success() {
        return Err(CredentialError::Keychain(name.into()));
    }
    let mut secret = output.stdout;
    if secret.last() == Some(&b'\n') {
        secret.pop();
    }
    if secret.len() as u64 > MAX_CREDENTIAL_BYTES {
        return Err(CredentialError::TooLarge {
            name: name.into(),
            bytes: secret.len() as u64,
            maximum: MAX_CREDENTIAL_BYTES,
        });
    }
    Ok(Zeroizing::new(secret))
}

#[cfg(not(target_os = "macos"))]
fn read_keychain(_prefix: &str, name: &str) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
    Err(CredentialError::Keychain(name.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_vault() -> (PathBuf, CredentialProvider) {
        let root = std::env::temp_dir().join(format!(
            "qwe-credentials-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let provider = CredentialProvider::new(Some(root.clone()), None);
        (root, provider)
    }

    #[test]
    fn loads_only_private_regular_credentials() {
        let (root, provider) = temporary_vault();
        let path = root.join("TEST_API_KEY");
        fs::write(&path, b"secret-value").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(provider.names().unwrap(), vec!["TEST_API_KEY"]);
        assert_eq!(
            provider.read("TEST_API_KEY").unwrap().as_slice(),
            b"secret-value"
        );

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            provider.read("TEST_API_KEY"),
            Err(CredentialError::InsecurePermissions)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_traversal_and_unconfigured_providers() {
        assert!(matches!(
            CredentialProvider::new(None, None).read("TEST_API_KEY"),
            Err(CredentialError::NotConfigured)
        ));
        let (_root, provider) = temporary_vault();
        assert!(matches!(
            provider.read("../secret"),
            Err(CredentialError::InvalidName)
        ));
    }

    #[test]
    fn rejects_symlinks_and_oversized_credentials() {
        let (root, provider) = temporary_vault();
        let target = root.join("TARGET");
        fs::write(&target, b"secret-value").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, root.join("LINK")).unwrap();
        assert!(matches!(
            provider.read("LINK"),
            Err(CredentialError::InvalidFile(_))
        ));

        let oversized = root.join("OVERSIZED");
        fs::write(&oversized, vec![b'x'; 16 * 1024 + 1]).unwrap();
        fs::set_permissions(&oversized, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            provider.read("OVERSIZED"),
            Err(CredentialError::TooLarge { .. })
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
