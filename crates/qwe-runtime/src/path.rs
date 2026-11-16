use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const SYS_OPENAT2: libc::c_long = 437;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const RESOLVE_BENEATH: u64 = 0x08;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

pub fn resolve_beneath(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let relative = if path.is_absolute() {
        path.strip_prefix(root).map_err(|_| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "path '{}' is outside root '{}'",
                    path.display(),
                    root.display()
                ),
            )
        })?
    } else {
        path
    };
    let root = fs::canonicalize(root)?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("path root '{}' is not a directory", root.display()),
        ));
    }
    let relative = if relative.as_os_str().is_empty() {
        Path::new(".")
    } else {
        relative
    };
    let resolved = resolve_openat2(&root, relative)?;
    if !resolved.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "resolved path '{}' escaped root '{}'",
                resolved.display(),
                root.display()
            ),
        ));
    }
    Ok(resolved)
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn resolve_openat2(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::{AsRawFd, FromRawFd};

    let root = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "root contains NUL"))?;
    let root_fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let root_file = unsafe { fs::File::from_raw_fd(root_fd) };
    let relative = CString::new(relative.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let how = OpenHow {
        flags: libc::O_PATH as u64 | libc::O_CLOEXEC as u64,
        mode: 0,
        resolve: RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS,
    };
    let descriptor = unsafe {
        libc::syscall(
            SYS_OPENAT2,
            root_file.as_raw_fd(),
            relative.as_ptr(),
            std::ptr::addr_of!(how),
            std::mem::size_of::<OpenHow>(),
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let descriptor = unsafe { fs::File::from_raw_fd(descriptor as i32) };
    fs::read_link(format!("/proc/self/fd/{}", descriptor.as_raw_fd()))
}

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
fn resolve_openat2(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let resolved = fs::canonicalize(root.join(relative))?;
    if resolved.starts_with(root) {
        Ok(resolved)
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "resolved path escaped root",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "qwe-resolve-beneath-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn resolves_inside_and_rejects_symlink_escape() {
        let root = temporary_root("link-escape");
        let inside = root.join("inside");
        fs::create_dir_all(&inside).unwrap();
        let file = inside.join("file");
        fs::write(&file, b"test").unwrap();
        std::os::unix::fs::symlink("inside", root.join("link")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("escape")).unwrap();

        assert_eq!(
            resolve_beneath(&root, &root.join("link/file")).unwrap(),
            fs::canonicalize(file).unwrap()
        );
        assert!(resolve_beneath(&root, &root.join("escape")).is_err());
        assert!(resolve_beneath(&root, &root.join("../outside")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn handles_absolute_paths_inside_and_outside_the_root() {
        let root = temporary_root("absolute");
        let file = root.join("inside");
        fs::write(&file, b"inside").unwrap();
        assert_eq!(
            resolve_beneath(&root, &file).unwrap(),
            fs::canonicalize(&file).unwrap()
        );
        assert!(resolve_beneath(&root, Path::new("/etc/passwd")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_symlink_loops_and_missing_paths() {
        let root = temporary_root("invalid");
        std::os::unix::fs::symlink("loop", root.join("loop")).unwrap();
        assert!(resolve_beneath(&root, &root.join("loop")).is_err());
        assert!(resolve_beneath(&root, &root.join("missing")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_deep_nesting_without_escaping() {
        let root = temporary_root("deep");
        let mut relative = PathBuf::new();
        for index in 0..64 {
            relative.push(format!("d{index}"));
        }
        let deep = root.join(&relative);
        fs::create_dir_all(&deep).unwrap();
        let file = deep.join("file");
        fs::write(&file, b"deep").unwrap();
        assert_eq!(
            resolve_beneath(&root, &file).unwrap(),
            fs::canonicalize(&file).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_non_directory_root() {
        let root = temporary_root("non-directory");
        let file = root.join("file");
        fs::write(&file, b"file").unwrap();
        assert_eq!(
            resolve_beneath(&file, Path::new(".")).unwrap_err().kind(),
            io::ErrorKind::NotADirectory
        );
        fs::remove_dir_all(root).unwrap();
    }
}
