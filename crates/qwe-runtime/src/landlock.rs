#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
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
const WRITE_ACCESS: u64 = LANDLOCK_ACCESS_FS_WRITE_FILE
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
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
        return Err(LandlockError::Syscall {
            operation: "no_new_privs setup",
            source: io::Error::last_os_error(),
        });
    }

    let ruleset = RulesetAttr {
        handled_access_fs: WRITE_ACCESS,
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
        allow_write_path(&ruleset, Path::new("/tmp"))?;
    }
    allow_write_path(&ruleset, Path::new("/dev"))?;
    if policy.workspace_read_write {
        allow_write_path(&ruleset, policy.workspace)?;
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
fn allow_write_path(ruleset: &std::fs::File, path: &Path) -> Result<(), LandlockError> {
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
        allowed_access: WRITE_ACCESS,
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
fn c_path(path: &Path) -> Result<std::ffi::CString, LandlockError> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| LandlockError::Syscall {
        operation: "path conversion",
        source: io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    #[test]
    fn write_mask_covers_file_and_directory_mutations() {
        assert_ne!(WRITE_ACCESS & LANDLOCK_ACCESS_FS_WRITE_FILE, 0);
        assert_ne!(WRITE_ACCESS & LANDLOCK_ACCESS_FS_REMOVE_FILE, 0);
        assert_ne!(WRITE_ACCESS & LANDLOCK_ACCESS_FS_MAKE_DIR, 0);
        assert_ne!(WRITE_ACCESS & LANDLOCK_ACCESS_FS_MAKE_SOCK, 0);
        assert_eq!(WRITE_ACCESS & (1 << 0), 0);
        assert_eq!(WRITE_ACCESS & (1 << 2), 0);
        assert_eq!(WRITE_ACCESS & (1 << 3), 0);
    }
}
