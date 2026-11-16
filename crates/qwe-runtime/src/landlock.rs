use std::io;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::Path;
use thiserror::Error;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_RULE_PATH_BENEATH: i32 = 1;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const LANDLOCK_ACCESS_FS_IOCTL_DEV: u64 = 1 << 15;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const WRITE_ACCESS_V1: u64 = LANDLOCK_ACCESS_FS_WRITE_FILE
    | LANDLOCK_ACCESS_FS_REMOVE_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_FILE
    | LANDLOCK_ACCESS_FS_MAKE_CHAR
    | LANDLOCK_ACCESS_FS_MAKE_DIR
    | LANDLOCK_ACCESS_FS_MAKE_REG
    | LANDLOCK_ACCESS_FS_MAKE_SOCK
    | LANDLOCK_ACCESS_FS_MAKE_FIFO
    | LANDLOCK_ACCESS_FS_MAKE_BLOCK
    | LANDLOCK_ACCESS_FS_MAKE_SYM;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;

#[derive(Debug, Error)]
pub enum LandlockError {
    #[error("Landlock is not supported by this kernel")]
    Unsupported,
    #[error("Landlock is not implemented for this architecture")]
    UnsupportedArchitecture,
    #[error("Landlock {operation} failed: {source}")]
    Syscall {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct LandlockPolicy<'a> {
    pub workspace: &'a Path,
    pub workspace_read_write: bool,
    pub private_tmp: bool,
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub fn abi_version() -> Result<u32, LandlockError> {
    let version = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::null::<RulesetAttr>(),
            0_usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if version > 0 {
        return Ok(version as u32);
    }
    let failure = io::Error::last_os_error();
    if matches!(
        failure.raw_os_error(),
        Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP)
    ) {
        Err(LandlockError::Unsupported)
    } else {
        Err(LandlockError::Syscall {
            operation: "ABI probe",
            source: failure,
        })
    }
}

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
pub fn abi_version() -> Result<u32, LandlockError> {
    Err(LandlockError::UnsupportedArchitecture)
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub fn apply(policy: &LandlockPolicy<'_>) -> Result<u32, LandlockError> {
    let abi = abi_version()?;
    let workspace_metadata =
        std::fs::metadata(policy.workspace).map_err(|source| LandlockError::Syscall {
            operation: "workspace validation",
            source,
        })?;
    if !workspace_metadata.is_dir() {
        return Err(LandlockError::Syscall {
            operation: "workspace validation",
            source: io::Error::new(io::ErrorKind::NotADirectory, "workspace is not a directory"),
        });
    }
    let write_access = write_access_for_abi(abi);
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
        return Err(LandlockError::Syscall {
            operation: "no_new_privs setup",
            source: io::Error::last_os_error(),
        });
    }

    let ruleset = RulesetAttr {
        handled_access_fs: write_access,
    };
    let ruleset_fd = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::addr_of!(ruleset),
            std::mem::size_of::<RulesetAttr>(),
            0_u32,
        )
    };
    if ruleset_fd < 0 {
        return Err(LandlockError::Syscall {
            operation: "ruleset creation",
            source: io::Error::last_os_error(),
        });
    }
    let ruleset = unsafe { std::fs::File::from_raw_fd(ruleset_fd as i32) };

    if policy.private_tmp {
        allow_write_path(&ruleset, Path::new("/tmp"), write_access)?;
    }
    allow_write_path(&ruleset, Path::new("/dev"), write_access)?;
    if policy.workspace_read_write {
        allow_write_path(&ruleset, policy.workspace, write_access)?;
    }

    if unsafe { libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset.as_raw_fd(), 0_u32) } < 0 {
        return Err(LandlockError::Syscall {
            operation: "self restriction",
            source: io::Error::last_os_error(),
        });
    }
    Ok(abi)
}

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
pub fn apply(_policy: &LandlockPolicy<'_>) -> Result<u32, LandlockError> {
    Err(LandlockError::UnsupportedArchitecture)
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn allow_write_path(
    ruleset: &std::fs::File,
    path: &Path,
    write_access: u64,
) -> Result<(), LandlockError> {
    let descriptor = unsafe {
        libc::open(
            c_path(path)?.as_ptr(),
            libc::O_PATH | libc::O_CLOEXEC | libc::O_DIRECTORY,
        )
    };
    if descriptor < 0 {
        return Err(LandlockError::Syscall {
            operation: "path open",
            source: io::Error::last_os_error(),
        });
    }
    let descriptor = unsafe { std::fs::File::from_raw_fd(descriptor) };
    let rule = PathBeneathAttr {
        allowed_access: write_access,
        parent_fd: descriptor.as_raw_fd(),
    };
    if unsafe {
        libc::syscall(
            SYS_LANDLOCK_ADD_RULE,
            ruleset.as_raw_fd(),
            LANDLOCK_RULE_PATH_BENEATH,
            std::ptr::addr_of!(rule),
            0_u32,
        )
    } < 0
    {
        return Err(LandlockError::Syscall {
            operation: "path rule",
            source: io::Error::last_os_error(),
        });
    }
    Ok(())
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn write_access_for_abi(abi: u32) -> u64 {
    let mut access = WRITE_ACCESS_V1;
    if abi >= 2 {
        access |= LANDLOCK_ACCESS_FS_REFER;
    }
    if abi >= 3 {
        access |= LANDLOCK_ACCESS_FS_TRUNCATE;
    }
    if abi >= 5 {
        access |= LANDLOCK_ACCESS_FS_IOCTL_DEV;
    }
    access
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn c_path(path: &Path) -> Result<std::ffi::CString, LandlockError> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| LandlockError::Syscall {
        operation: "path conversion",
        source: io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"),
    })
}

#[cfg(all(
    test,
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod tests {
    use super::*;

    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    #[test]
    fn write_mask_grows_with_supported_landlock_abi() {
        let v1 = write_access_for_abi(1);
        assert_ne!(v1 & LANDLOCK_ACCESS_FS_WRITE_FILE, 0);
        assert_ne!(v1 & LANDLOCK_ACCESS_FS_REMOVE_FILE, 0);
        assert_ne!(v1 & LANDLOCK_ACCESS_FS_MAKE_DIR, 0);
        assert_ne!(v1 & LANDLOCK_ACCESS_FS_MAKE_SOCK, 0);
        assert_eq!(v1 & (1 << 0), 0);
        assert_eq!(v1 & (1 << 2), 0);
        assert_eq!(v1 & (1 << 3), 0);
        assert_eq!(v1 & LANDLOCK_ACCESS_FS_REFER, 0);
        assert_eq!(v1 & LANDLOCK_ACCESS_FS_TRUNCATE, 0);
        assert_eq!(v1 & LANDLOCK_ACCESS_FS_IOCTL_DEV, 0);

        let v2 = write_access_for_abi(2);
        assert_ne!(v2 & LANDLOCK_ACCESS_FS_REFER, 0);
        assert_eq!(v2 & LANDLOCK_ACCESS_FS_TRUNCATE, 0);

        let v3 = write_access_for_abi(3);
        assert_ne!(v3 & LANDLOCK_ACCESS_FS_TRUNCATE, 0);
        assert_eq!(v3 & LANDLOCK_ACCESS_FS_IOCTL_DEV, 0);

        let v4 = write_access_for_abi(4);
        assert_eq!(v4 & LANDLOCK_ACCESS_FS_IOCTL_DEV, 0);

        let v5 = write_access_for_abi(5);
        assert_ne!(v5 & LANDLOCK_ACCESS_FS_IOCTL_DEV, 0);
        assert_eq!(write_access_for_abi(10), v5);
    }
}
