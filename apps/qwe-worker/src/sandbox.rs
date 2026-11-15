use qwe_core::{
    SANDBOX_SECCOMP_STRICT_PROFILE, SandboxHomeMode, SandboxLandlockMode, SandboxNetworkMode,
    SandboxProcessMode, SandboxProfile, SandboxSeccompMode, SandboxWorkspaceMode,
};
use qwe_protocol::ProtocolErrorDetails;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::{FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BubblewrapSupport {
    pub version: String,
    pub path: PathBuf,
    pub probe_time_ms: i64,
    pub landlock_abi: Option<u32>,
    pub fd_bind: bool,
}

#[derive(Debug, Clone)]
pub struct BubblewrapProbeFailure {
    pub message: String,
    pub details: ProtocolErrorDetails,
}

const REQUIRED_BUBBLEWRAP_OPTIONS: &[&str] = &[
    "--as-pid-1",
    "--bind",
    "--cap-drop",
    "--chdir",
    "--clearenv",
    "--dev",
    "--die-with-parent",
    "--dir",
    "--json-status-fd",
    "--proc",
    "--ro-bind",
    "--ro-bind-try",
    "--seccomp",
    "--setenv",
    "--symlink",
    "--tmpfs",
    "--unshare-ipc",
    "--unshare-net",
    "--unshare-pid",
    "--unshare-user",
    "--unshare-uts",
];

pub fn probe_bubblewrap(
    landlock: SandboxLandlockMode,
) -> Result<BubblewrapSupport, BubblewrapProbeFailure> {
    let mut evidence = ProbeEvidence::collect();
    let executable = bubblewrap_executable();
    evidence.backend_path = executable.display().to_string();
    let version = Command::new(&executable)
        .arg("--version")
        .output()
        .map_err(|failure| {
            evidence.stderr = sanitize_stderr(failure.to_string().as_bytes());
            probe_failure(
                "bwrap_missing",
                format!("bubblewrap is unavailable: {failure}"),
                &evidence,
            )
        })?;
    if !version.status.success() {
        evidence.stderr = sanitize_stderr(&version.stderr);
        return Err(probe_failure(
            "bwrap_version_incompatible",
            "bubblewrap --version failed",
            &evidence,
        ));
    }
    evidence.backend_version = String::from_utf8_lossy(&version.stdout).trim().to_string();
    let help = Command::new(&executable)
        .arg("--help")
        .output()
        .map_err(|failure| {
            evidence.stderr = sanitize_stderr(failure.to_string().as_bytes());
            probe_failure(
                "bwrap_version_incompatible",
                format!("failed to inspect bubblewrap features: {failure}"),
                &evidence,
            )
        })?;
    if !help.status.success() {
        evidence.stderr = sanitize_stderr(&help.stderr);
        return Err(probe_failure(
            "bwrap_version_incompatible",
            "bubblewrap --help failed",
            &evidence,
        ));
    }
    let mut help_text = String::from_utf8_lossy(&help.stdout).into_owned();
    help_text.push_str(&String::from_utf8_lossy(&help.stderr));
    let missing = missing_bubblewrap_options(&help_text);
    if !missing.is_empty() {
        evidence.stderr = format!(
            "bubblewrap is missing required option(s): {}",
            missing.join(", ")
        );
        return Err(probe_failure(
            "bwrap_version_incompatible",
            "bubblewrap does not support the QWE sandbox contract",
            &evidence,
        ));
    }
    let has_fd_bind = bubblewrap_supports_option(&help_text, "--bind-fd")
        && bubblewrap_supports_option(&help_text, "--ro-bind-fd");
    let fd_bind = has_fd_bind && probe_fd_bind(&executable).unwrap_or(false);
    let landlock_abi = match landlock {
        SandboxLandlockMode::Off => None,
        SandboxLandlockMode::BestEffort => match qwe_runtime::landlock_abi_version() {
            Ok(version) => Some(version),
            Err(qwe_runtime::LandlockError::Unsupported)
            | Err(qwe_runtime::LandlockError::UnsupportedArchitecture) => None,
            Err(failure) => {
                evidence.stderr = sanitize_stderr(failure.to_string().as_bytes());
                return Err(probe_failure(
                    "landlock_probe_failed",
                    "failed to probe Landlock defense in depth",
                    &evidence,
                ));
            }
        },
        SandboxLandlockMode::Required { min_abi } => match qwe_runtime::landlock_abi_version() {
            Ok(version) if version >= min_abi => Some(version),
            Ok(version) => {
                evidence.landlock_abi = Some(version);
                evidence.stderr =
                    format!("Landlock ABI {version} is lower than required ABI {min_abi}");
                return Err(probe_failure(
                    "landlock_required_unavailable",
                    format!("Landlock ABI {version} is lower than required ABI {min_abi}"),
                    &evidence,
                ));
            }
            Err(qwe_runtime::LandlockError::Unsupported)
            | Err(qwe_runtime::LandlockError::UnsupportedArchitecture) => {
                evidence.stderr = format!("Landlock is unavailable; required ABI is {min_abi}");
                return Err(probe_failure(
                    "landlock_required_unavailable",
                    format!("Landlock is unavailable; required ABI is {min_abi}"),
                    &evidence,
                ));
            }
            Err(failure) => {
                evidence.stderr = sanitize_stderr(failure.to_string().as_bytes());
                return Err(probe_failure(
                    "landlock_probe_failed",
                    "failed to probe required Landlock support",
                    &evidence,
                ));
            }
        },
    };
    evidence.landlock_abi = landlock_abi;
    if landlock_abi.is_some()
        && let Err(failure) = sandbox_init_for_landlock(landlock)
    {
        evidence.stderr = sanitize_stderr(failure.to_string().as_bytes());
        return Err(probe_failure(
            "sandbox_init_unavailable",
            "Landlock is available but qwe-sandbox-init is not usable",
            &evidence,
        ));
    }
    let mut child = Command::new(&executable)
        .args([
            "--ro-bind",
            "/usr",
            "/usr",
            "--symlink",
            "usr/bin",
            "/bin",
            "--ro-bind-try",
            "/lib",
            "/lib",
            "--symlink",
            "usr/lib64",
            "/lib64",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--unshare-user",
            "--unshare-pid",
            "--as-pid-1",
            "--die-with-parent",
            "--",
            "/usr/bin/true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|failure| {
            evidence.stderr = sanitize_stderr(failure.to_string().as_bytes());
            probe_failure(
                "bwrap_missing",
                format!("failed to start bubblewrap probe: {failure}"),
                &evidence,
            )
        })?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return Ok(BubblewrapSupport {
                    version: evidence.backend_version,
                    path: executable,
                    probe_time_ms: evidence.probe_time_ms,
                    landlock_abi,
                    fd_bind,
                });
            }
            Ok(Some(status)) => {
                evidence.stderr = read_stderr(&mut child);
                let reason = classify_probe_failure(&evidence);
                return Err(probe_failure(
                    reason,
                    format!("bubblewrap user/PID namespace probe exited with {status}"),
                    &evidence,
                ));
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                evidence.stderr = read_stderr(&mut child);
                return Err(probe_failure(
                    "probe_timeout",
                    "bubblewrap namespace probe timed out",
                    &evidence,
                ));
            }
            Err(failure) => {
                let _ = child.kill();
                let _ = child.wait();
                evidence.stderr = read_stderr(&mut child);
                return Err(probe_failure(
                    "unknown",
                    format!("bubblewrap namespace probe failed: {failure}"),
                    &evidence,
                ));
            }
        }
    }
}

pub fn sandbox_init_for_landlock(mode: SandboxLandlockMode) -> io::Result<Option<PathBuf>> {
    match mode {
        SandboxLandlockMode::Off => Ok(None),
        SandboxLandlockMode::BestEffort | SandboxLandlockMode::Required { .. } => {
            let abi = qwe_runtime::landlock_abi_version();
            let available = match (mode, abi) {
                (SandboxLandlockMode::Off, _) => false,
                (SandboxLandlockMode::BestEffort, Ok(_)) => true,
                (
                    SandboxLandlockMode::BestEffort,
                    Err(qwe_runtime::LandlockError::Unsupported)
                    | Err(qwe_runtime::LandlockError::UnsupportedArchitecture),
                ) => false,
                (SandboxLandlockMode::Required { min_abi }, Ok(version)) if version >= min_abi => {
                    true
                }
                (SandboxLandlockMode::Required { min_abi }, Ok(version)) => {
                    return Err(io::Error::other(format!(
                        "Landlock ABI {version} is lower than required ABI {min_abi}"
                    )));
                }
                (
                    SandboxLandlockMode::Required { min_abi },
                    Err(qwe_runtime::LandlockError::Unsupported)
                    | Err(qwe_runtime::LandlockError::UnsupportedArchitecture),
                ) => {
                    return Err(io::Error::other(format!(
                        "Landlock is unavailable; required ABI is {min_abi}"
                    )));
                }
                (_, Err(failure)) => return Err(io::Error::other(failure)),
            };
            if !available {
                return Ok(None);
            }
            let init = std::env::current_exe()?.with_file_name("qwe-sandbox-init");
            let metadata = fs::metadata(&init).map_err(|failure| {
                io::Error::new(
                    failure.kind(),
                    format!(
                        "required Landlock helper '{}' is unavailable: {failure}",
                        init.display()
                    ),
                )
            })?;
            if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "required Landlock helper '{}' is not executable",
                        init.display()
                    ),
                ));
            }
            Ok(Some(init))
        }
    }
}

fn missing_bubblewrap_options(help: &str) -> Vec<&'static str> {
    REQUIRED_BUBBLEWRAP_OPTIONS
        .iter()
        .copied()
        .filter(|option| {
            !help.split_whitespace().any(|token| {
                token == *option
                    || token
                        .strip_prefix(option)
                        .is_some_and(|suffix| suffix.starts_with('='))
            })
        })
        .collect()
}

fn bubblewrap_supports_option(help: &str, option: &str) -> bool {
    help.split_whitespace().any(|token| {
        token == option
            || token
                .strip_prefix(option)
                .is_some_and(|suffix| suffix.starts_with('='))
    })
}

fn probe_fd_bind(executable: &Path) -> io::Result<bool> {
    use std::os::unix::ffi::OsStrExt;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let root =
        std::env::temp_dir().join(format!("qwe-bwrap-fd-bind-{}-{nonce}", std::process::id()));
    fs::create_dir(&root)?;
    fs::write(root.join("marker"), b"QWE_FD_BIND_OK")?;
    let root_c = std::ffi::CString::new(root.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let descriptor = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let failure = io::Error::last_os_error();
        let _ = fs::remove_dir_all(&root);
        return Err(failure);
    }
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
    {
        let failure = io::Error::last_os_error();
        unsafe {
            libc::close(descriptor);
        }
        let _ = fs::remove_dir_all(&root);
        return Err(failure);
    }
    let fd_arg = descriptor.to_string();
    let output = Command::new(executable)
        .args([
            "--ro-bind",
            "/usr",
            "/usr",
            "--symlink",
            "usr/bin",
            "/bin",
            "--ro-bind-try",
            "/lib",
            "/lib",
            "--symlink",
            "usr/lib64",
            "/lib64",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--unshare-user",
            "--unshare-pid",
            "--as-pid-1",
            "--die-with-parent",
            "--ro-bind-fd",
            &fd_arg,
            "/qwe-fd-bind",
            "--",
            "/bin/cat",
            "/qwe-fd-bind/marker",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    unsafe {
        libc::close(descriptor);
    }
    let _ = fs::remove_dir_all(&root);
    Ok(output.is_ok_and(|output| output.status.success() && output.stdout == b"QWE_FD_BIND_OK"))
}

#[derive(Debug, Clone)]
struct ProbeEvidence {
    backend_path: String,
    backend_version: String,
    probe_time_ms: i64,
    stderr: String,
    max_user_namespaces: Option<String>,
    unprivileged_userns_clone: Option<String>,
    apparmor_restrict_unprivileged_userns: Option<String>,
    landlock_abi: Option<u32>,
    container: bool,
}

impl ProbeEvidence {
    fn collect() -> Self {
        Self {
            backend_path: bubblewrap_executable().display().to_string(),
            backend_version: "unavailable".into(),
            probe_time_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis() as i64),
            stderr: String::new(),
            max_user_namespaces: read_sysctl("/proc/sys/user/max_user_namespaces"),
            unprivileged_userns_clone: read_sysctl("/proc/sys/kernel/unprivileged_userns_clone"),
            apparmor_restrict_unprivileged_userns: read_sysctl(
                "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
            ),
            landlock_abi: None,
            container: Path::new("/.dockerenv").exists()
                || Path::new("/run/.containerenv").exists(),
        }
    }
}

fn probe_failure(
    reason: &str,
    message: impl Into<String>,
    evidence: &ProbeEvidence,
) -> BubblewrapProbeFailure {
    let failed_stage = match reason {
        "bwrap_missing" => "resolve",
        "bwrap_version_incompatible" => "version",
        "landlock_probe_failed" | "landlock_required_unavailable" | "sandbox_init_unavailable" => {
            "landlock_init"
        }
        "uid_map_denied" => "uid_map",
        "mount_denied" => "mount",
        "userns_disabled" | "userns_create_denied" => "userns",
        "probe_timeout" => "probe_timeout",
        _ => "probe",
    };
    BubblewrapProbeFailure {
        message: message.into(),
        details: ProtocolErrorDetails {
            reason: reason.into(),
            diagnostics: json!({
                "backend_version": evidence.backend_version,
                "backend_path": evidence.backend_path,
                "probe_time_ms": evidence.probe_time_ms,
                "environment": if evidence.container { "container" } else { "host" },
                "failed_stage": failed_stage,
                "target": mount_target(&evidence.stderr),
                "landlock_abi": evidence.landlock_abi,
                "sysctls": {
                    "max_user_namespaces": evidence.max_user_namespaces,
                    "unprivileged_userns_clone": evidence.unprivileged_userns_clone,
                    "apparmor_restrict_unprivileged_userns": evidence.apparmor_restrict_unprivileged_userns
                },
                "stderr": evidence.stderr
            }),
        },
    }
}

fn mount_target(stderr: &str) -> Option<String> {
    let (_, remainder) = stderr.split_once(" on ")?;
    let target = remainder.split(':').next()?.trim();
    let target = target.strip_prefix("/newroot").unwrap_or(target);
    target.starts_with('/').then(|| target.to_string())
}

fn classify_probe_failure(evidence: &ProbeEvidence) -> &'static str {
    if evidence.max_user_namespaces.as_deref() == Some("0")
        || evidence.unprivileged_userns_clone.as_deref() == Some("0")
    {
        return "userns_disabled";
    }
    let stderr = evidence.stderr.to_ascii_lowercase();
    if stderr.contains("uid map") {
        return "uid_map_denied";
    }
    if stderr.contains("mount") && stderr.contains("operation not permitted") {
        return "mount_denied";
    }
    if stderr.contains("create new namespace") || stderr.contains("user namespaces") {
        return "userns_create_denied";
    }
    "unknown"
}

#[must_use]
pub fn bubblewrap_executable() -> PathBuf {
    std::env::var_os("QWE_BWRAP_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("bwrap"))
}

fn read_sysctl(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
}

fn read_stderr(child: &mut std::process::Child) -> String {
    let mut bytes = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_end(&mut bytes);
    }
    sanitize_stderr(&bytes)
}

fn sanitize_stderr(bytes: &[u8]) -> String {
    let mut value = String::from_utf8_lossy(bytes).replace('\0', "");
    if let Some(home) = std::env::var_os("HOME").and_then(|value| value.into_string().ok())
        && !home.is_empty()
    {
        value = value.replace(&home, "$HOME");
    }
    const MAX_STDERR_BYTES: usize = 4096;
    if value.len() > MAX_STDERR_BYTES {
        let mut end = MAX_STDERR_BYTES;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    value
}

pub struct BubblewrapInvocation {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
}

pub struct BubblewrapOptions<'a> {
    pub status_fd: i32,
    pub seccomp_fd: Option<i32>,
    pub credential_present: bool,
    pub term: Option<&'a str>,
    pub sandbox_init: Option<&'a Path>,
}

pub fn bubblewrap_invocation(
    profile: &SandboxProfile,
    program: &Path,
    arguments: &[String],
    requested_cwd: Option<&Path>,
    options: BubblewrapOptions<'_>,
) -> io::Result<BubblewrapInvocation> {
    let workspace = std::fs::canonicalize(&profile.workspace)?;
    if !workspace.is_dir() {
        return Err(invalid_input(format!(
            "sandbox workspace '{}' is not a directory",
            workspace.display()
        )));
    }
    let cwd = match requested_cwd {
        Some(path) => qwe_runtime::resolve_beneath(&profile.workspace, path)?,
        None => workspace.clone(),
    };
    if !cwd.starts_with(&workspace) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "sandbox cwd '{}' is outside workspace '{}'",
                cwd.display(),
                workspace.display()
            ),
        ));
    }
    let program = std::fs::canonicalize(program)?;
    if !program_is_visible(&program, &workspace) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "program '{}' is outside the sandbox mount policy",
                program.display()
            ),
        ));
    }

    let mut args = vec![
        "--unshare-user".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--cap-drop".into(),
        "ALL".into(),
        "--die-with-parent".into(),
        "--json-status-fd".into(),
        options.status_fd.to_string(),
        "--clearenv".into(),
        "--setenv".into(),
        "PATH".into(),
        "/usr/bin:/bin:/usr/sbin:/sbin".into(),
        "--setenv".into(),
        "LANG".into(),
        "C.UTF-8".into(),
    ];
    if let Some(term) = options.term {
        args.extend(["--setenv".into(), "TERM".into(), term.into()]);
    }
    if let Some(descriptor) = options.seccomp_fd {
        args.extend(["--seccomp".into(), descriptor.to_string()]);
    }
    for system_path in [
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/opt",
        "/nix/store",
    ] {
        push_option(&mut args, "--ro-bind-try", system_path, system_path);
    }
    if let Some(sandbox_init) = options.sandbox_init {
        let sandbox_init = std::fs::canonicalize(sandbox_init)?;
        push_option(
            &mut args,
            "--ro-bind",
            &path_string(&sandbox_init)?,
            "/qwe-sandbox-init",
        );
    }
    args.extend([
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
    ]);
    if profile.private_tmp {
        args.extend(["--tmpfs".into(), "/tmp".into()]);
    }
    args.extend(["--dir".into(), "/etc".into()]);
    for config_path in [
        "/etc/ld.so.cache",
        "/etc/ld.so.conf",
        "/etc/passwd",
        "/etc/group",
        "/etc/nsswitch.conf",
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/services",
        "/etc/protocols",
        "/etc/localtime",
        "/etc/ssl",
        "/etc/ca-certificates",
    ] {
        push_option(&mut args, "--ro-bind-try", config_path, config_path);
    }

    match profile.network {
        SandboxNetworkMode::IsolatedLoopback => args.push("--unshare-net".into()),
        SandboxNetworkMode::Inherit => {}
    }
    if profile.process_namespace == SandboxProcessMode::Isolated {
        args.extend(["--unshare-pid".into(), "--as-pid-1".into()]);
    }

    push_bind_option(
        &mut args,
        match profile.workspace_mode {
            SandboxWorkspaceMode::ReadOnly => "--ro-bind",
            SandboxWorkspaceMode::ReadWrite => "--bind",
        },
        &workspace,
    )?;
    for protected_path in &profile.protected_paths {
        let path = qwe_runtime::resolve_beneath(&profile.workspace, protected_path)?;
        if !path.starts_with(&workspace) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "protected path '{}' is outside workspace '{}'",
                    path.display(),
                    workspace.display()
                ),
            ));
        }
        push_bind_option(&mut args, "--ro-bind", &path)?;
    }

    let home = match profile.home {
        SandboxHomeMode::Hidden => PathBuf::from("/nonexistent"),
        SandboxHomeMode::ReadOnly => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| invalid_input("HOME is unavailable for sandbox profile"))?;
            let home = std::fs::canonicalize(home)?;
            push_bind_option(&mut args, "--ro-bind", &home)?;
            home
        }
    };
    args.extend(["--setenv".into(), "HOME".into(), path_string(&home)?]);
    push_path_option(&mut args, "--chdir", &cwd)?;

    if options.credential_present {
        args.extend(["--setenv".into(), "QWE_CREDENTIAL_FD".into(), "3".into()]);
    }
    args.push("--".into());
    if options.sandbox_init.is_some() {
        args.push("/qwe-sandbox-init".into());
        args.extend([
            "--workspace".into(),
            path_string(&workspace)?,
            "--workspace-mode".into(),
            match profile.workspace_mode {
                SandboxWorkspaceMode::ReadOnly => "read-only",
                SandboxWorkspaceMode::ReadWrite => "read-write",
            }
            .into(),
        ]);
        if profile.private_tmp {
            args.push("--private-tmp".into());
        }
        args.push("--".into());
    }
    args.push(path_string(&program)?);
    args.extend(arguments.iter().cloned());

    Ok(BubblewrapInvocation {
        executable: bubblewrap_executable(),
        arguments: args,
    })
}

pub fn write_seccomp_filter(path: &Path, mode: &SandboxSeccompMode) -> io::Result<()> {
    let filters = seccomp_filter_program(mode)?;
    let filter_bytes = unsafe {
        std::slice::from_raw_parts(
            filters.as_ptr().cast::<u8>(),
            std::mem::size_of_val(filters.as_slice()),
        )
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(filter_bytes)?;
    file.flush()
}

pub fn seccomp_filter_hash(mode: &SandboxSeccompMode) -> io::Result<String> {
    let filters = seccomp_filter_program(mode)?;
    let bytes = filter_bytes(&filters);
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn create_seccomp_filter_fd(mode: &SandboxSeccompMode) -> io::Result<RawFd> {
    let filters = seccomp_filter_program(mode)?;
    let bytes = filter_bytes(&filters);
    let mut descriptors = [-1_i32; 2];
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut writer = unsafe { std::fs::File::from_raw_fd(descriptors[1]) };
    if let Err(failure) = writer.write_all(bytes).and_then(|()| writer.flush()) {
        unsafe {
            libc::close(descriptors[0]);
        }
        return Err(failure);
    }
    drop(writer);
    let inherited = unsafe { libc::fcntl(descriptors[0], libc::F_DUPFD_CLOEXEC, 10) };
    unsafe {
        libc::close(descriptors[0]);
    }
    if inherited < 0 {
        return Err(io::Error::last_os_error());
    }
    let flags = unsafe { libc::fcntl(inherited, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(inherited, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
    {
        let failure = io::Error::last_os_error();
        unsafe {
            libc::close(inherited);
        }
        return Err(failure);
    }
    Ok(inherited)
}

fn filter_bytes(filters: &[libc::sock_filter]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            filters.as_ptr().cast::<u8>(),
            std::mem::size_of_val(filters),
        )
    }
}

fn seccomp_filter_program(mode: &SandboxSeccompMode) -> io::Result<Vec<libc::sock_filter>> {
    #[cfg(target_arch = "x86_64")]
    const AUDIT_ARCH: u32 = 0xc000_003e;
    #[cfg(target_arch = "x86_64")]
    const SYS_IOCTL: u32 = 16;
    #[cfg(target_arch = "x86_64")]
    const SYS_PTRACE: u32 = 101;
    #[cfg(target_arch = "x86_64")]
    const SYS_KEYCTL: u32 = 250;
    #[cfg(target_arch = "x86_64")]
    const SYS_ADD_KEY: u32 = 248;
    #[cfg(target_arch = "x86_64")]
    const SYS_REQUEST_KEY: u32 = 249;
    #[cfg(target_arch = "x86_64")]
    const SYS_BPF: u32 = 321;
    #[cfg(target_arch = "x86_64")]
    const SYS_USERFAULTFD: u32 = 323;
    #[cfg(target_arch = "x86_64")]
    const SYS_IO_URING_SETUP: u32 = 425;
    #[cfg(target_arch = "aarch64")]
    const AUDIT_ARCH: u32 = 0xc000_00b7;
    #[cfg(target_arch = "aarch64")]
    const SYS_IOCTL: u32 = 29;
    #[cfg(target_arch = "aarch64")]
    const SYS_PTRACE: u32 = 117;
    #[cfg(target_arch = "aarch64")]
    const SYS_KEYCTL: u32 = 219;
    #[cfg(target_arch = "aarch64")]
    const SYS_ADD_KEY: u32 = 217;
    #[cfg(target_arch = "aarch64")]
    const SYS_REQUEST_KEY: u32 = 218;
    #[cfg(target_arch = "aarch64")]
    const SYS_BPF: u32 = 280;
    #[cfg(target_arch = "aarch64")]
    const SYS_USERFAULTFD: u32 = 282;
    #[cfg(target_arch = "aarch64")]
    const SYS_IO_URING_SETUP: u32 = 425;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "sandbox seccomp filter is not implemented for this architecture",
        ));
    }
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        const SECCOMP_DATA_NR: u32 = 0;
        const SECCOMP_DATA_ARCH: u32 = 4;
        const SECCOMP_DATA_ARG1: u32 = 24;
        const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
        let deny = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
        let mut filters = vec![
            bpf(
                libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
                SECCOMP_DATA_ARCH,
            ),
            bpf_jump(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                AUDIT_ARCH,
                1,
                0,
            ),
            bpf_return(SECCOMP_RET_KILL_PROCESS),
            bpf(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, SECCOMP_DATA_NR),
        ];
        if matches!(
            mode,
            SandboxSeccompMode::Named { profile }
                if profile == SANDBOX_SECCOMP_STRICT_PROFILE
        ) {
            for syscall in [
                SYS_PTRACE,
                SYS_BPF,
                SYS_KEYCTL,
                SYS_ADD_KEY,
                SYS_REQUEST_KEY,
                SYS_USERFAULTFD,
            ] {
                filters.push(bpf_jump(
                    libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                    syscall,
                    0,
                    1,
                ));
                filters.push(bpf_return(deny));
            }
            filters.push(bpf_jump(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                SYS_IO_URING_SETUP,
                0,
                1,
            ));
            filters.push(bpf_return(libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32));
        } else if !matches!(mode, SandboxSeccompMode::None) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "only the named seccomp profile 'strict' is supported",
            ));
        }
        filters.extend([
            bpf_jump(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, SYS_IOCTL, 0, 4),
            bpf(
                libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
                SECCOMP_DATA_ARG1,
            ),
            bpf_jump(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                libc::TIOCSTI as u32,
                0,
                1,
            ),
            bpf_return(deny),
            bpf_return(libc::SECCOMP_RET_ALLOW),
            bpf_return(libc::SECCOMP_RET_ALLOW),
        ]);
        Ok(filters)
    }
}

fn bpf(code: u32, value: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt: 0,
        jf: 0,
        k: value,
    }
}

fn bpf_jump(code: u32, value: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k: value,
    }
}

fn bpf_return(value: u32) -> libc::sock_filter {
    bpf(libc::BPF_RET | libc::BPF_K, value)
}

fn program_is_visible(program: &Path, workspace: &Path) -> bool {
    program.starts_with(workspace)
        || [
            "/usr",
            "/bin",
            "/sbin",
            "/lib",
            "/lib64",
            "/opt",
            "/nix/store",
        ]
        .iter()
        .any(|root| program.starts_with(root))
}

fn push_option(args: &mut Vec<String>, option: &str, source: &str, destination: &str) {
    args.push(option.into());
    args.push(source.into());
    args.push(destination.into());
}

fn push_path_option(args: &mut Vec<String>, option: &str, path: &Path) -> io::Result<()> {
    args.push(option.into());
    args.push(path_string(path)?);
    Ok(())
}

fn push_bind_option(args: &mut Vec<String>, option: &str, path: &Path) -> io::Result<()> {
    let path = path_string(path)?;
    args.push(option.into());
    args.push(path.clone());
    args.push(path);
    Ok(())
}

fn path_string(path: &Path) -> io::Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| invalid_input(format!("sandbox path '{}' is not UTF-8", path.display())))
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    fn evidence(stderr: &str) -> ProbeEvidence {
        ProbeEvidence {
            backend_path: "/opt/local/bin/bwrap".into(),
            backend_version: "bubblewrap test".into(),
            probe_time_ms: 1,
            stderr: stderr.into(),
            max_user_namespaces: Some("1000".into()),
            unprivileged_userns_clone: Some("1".into()),
            apparmor_restrict_unprivileged_userns: Some("0".into()),
            landlock_abi: Some(1),
            container: false,
        }
    }

    #[test]
    fn feature_probe_requires_every_production_bubblewrap_option() {
        let complete = REQUIRED_BUBBLEWRAP_OPTIONS.join(" ");
        assert!(missing_bubblewrap_options(&complete).is_empty());

        let incomplete = REQUIRED_BUBBLEWRAP_OPTIONS
            .iter()
            .filter(|option| **option != "--json-status-fd")
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            missing_bubblewrap_options(&incomplete),
            vec!["--json-status-fd"]
        );
    }

    #[test]
    fn classifies_userns_disabled_from_sysctl_evidence() {
        let mut evidence = evidence("No permissions to create new namespace");
        evidence.max_user_namespaces = Some("0".into());
        assert_eq!(classify_probe_failure(&evidence), "userns_disabled");

        evidence.max_user_namespaces = Some("1000".into());
        evidence.unprivileged_userns_clone = Some("0".into());
        assert_eq!(classify_probe_failure(&evidence), "userns_disabled");
    }

    #[test]
    fn classifies_uid_map_denial_without_guessing_apparmor() {
        let mut evidence = evidence("bwrap: setting up uid map: Permission denied");
        evidence.apparmor_restrict_unprivileged_userns = Some("1".into());
        assert_eq!(classify_probe_failure(&evidence), "uid_map_denied");
    }

    #[test]
    fn classifies_mount_denial_with_container_as_context_only() {
        let mut evidence =
            evidence("bwrap: Can't mount proc on /newroot/proc: Operation not permitted");
        evidence.container = true;
        assert_eq!(classify_probe_failure(&evidence), "mount_denied");
        let failure = probe_failure("mount_denied", "mount failed", &evidence);
        assert_eq!(failure.details.diagnostics["target"], "/proc");
    }

    #[test]
    fn landlock_init_failure_reports_the_init_stage() {
        let failure = probe_failure(
            "sandbox_init_unavailable",
            "Landlock helper is missing",
            &evidence("required Landlock helper is unavailable"),
        );
        assert_eq!(failure.details.reason, "sandbox_init_unavailable");
        assert_eq!(failure.details.diagnostics["failed_stage"], "landlock_init");
        assert_eq!(failure.details.diagnostics["landlock_abi"], 1);
    }

    #[test]
    fn classifies_namespace_creation_denial() {
        let evidence = evidence("bwrap: No permissions to create new namespace");
        assert_eq!(classify_probe_failure(&evidence), "userns_create_denied");
    }

    #[test]
    fn unknown_failure_keeps_stderr_for_diagnostics() {
        let evidence = evidence("unrecognized bubblewrap failure");
        let failure = probe_failure("unknown", "probe failed", &evidence);
        assert_eq!(failure.details.reason, "unknown");
        assert_eq!(
            failure.details.diagnostics["stderr"],
            "unrecognized bubblewrap failure"
        );
    }

    #[test]
    fn strict_seccomp_filter_is_versioned_by_content_hash() {
        let none = seccomp_filter_hash(&SandboxSeccompMode::None).unwrap();
        let strict = seccomp_filter_hash(&SandboxSeccompMode::Named {
            profile: "strict".into(),
        })
        .unwrap();
        assert_eq!(none.len(), 64);
        assert_eq!(strict.len(), 64);
        assert_ne!(none, strict);

        assert!(
            seccomp_filter_hash(&SandboxSeccompMode::Named {
                profile: "unknown".into(),
            })
            .is_err()
        );
    }

    #[test]
    fn strict_seccomp_nested_bubblewrap_gate() {
        if probe_bubblewrap(SandboxLandlockMode::Off).is_err() {
            eprintln!("SKIP: bubblewrap namespaces are unavailable");
            return;
        }
        let seccomp_fd = match create_seccomp_filter_fd(&SandboxSeccompMode::Named {
            profile: SANDBOX_SECCOMP_STRICT_PROFILE.into(),
        }) {
            Ok(descriptor) => descriptor,
            Err(failure) if failure.kind() == io::ErrorKind::Unsupported => {
                eprintln!("SKIP: strict seccomp is unsupported on this architecture");
                return;
            }
            Err(failure) => panic!("failed to create strict seccomp filter: {failure}"),
        };
        let seccomp_arg = seccomp_fd.to_string();
        let mut command = Command::new(bubblewrap_executable());
        command
            .args([
                "--ro-bind",
                "/usr",
                "/usr",
                "--symlink",
                "usr/bin",
                "/bin",
                "--ro-bind-try",
                "/lib",
                "/lib",
                "--symlink",
                "usr/lib64",
                "/lib64",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--tmpfs",
                "/tmp",
                "--unshare-user",
                "--unshare-pid",
                "--as-pid-1",
                "--die-with-parent",
                "--seccomp",
                &seccomp_arg,
                "--",
                "/usr/bin/bwrap",
                "--ro-bind",
                "/usr",
                "/usr",
                "--symlink",
                "usr/bin",
                "/bin",
                "--ro-bind-try",
                "/lib",
                "/lib",
                "--symlink",
                "usr/lib64",
                "/lib64",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--unshare-user",
                "--unshare-pid",
                "--as-pid-1",
                "--die-with-parent",
                "--",
                "/bin/true",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = match command.spawn() {
            Ok(child) => child,
            Err(failure) => {
                unsafe {
                    libc::close(seccomp_fd);
                }
                panic!("failed to start outer bubblewrap: {failure}");
            }
        };
        unsafe {
            libc::close(seccomp_fd);
        }
        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("nested bwrap status={} stderr={stderr}", output.status);
        assert!(
            output.status.success() || !stderr.is_empty(),
            "nested bubblewrap neither succeeded nor reported a diagnostic"
        );
    }

    #[test]
    fn seccomp_socket_denial_preserves_inherited_socket() {
        if probe_bubblewrap(SandboxLandlockMode::Off).is_err() {
            eprintln!("SKIP: bubblewrap namespaces are unavailable");
            return;
        }
        #[cfg(target_arch = "x86_64")]
        const SYS_SOCKET: u32 = 41;
        #[cfg(target_arch = "aarch64")]
        const SYS_SOCKET: u32 = 198;
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            eprintln!("SKIP: socket-denial filter is unsupported on this architecture");
            return;
        }
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        {
            const SECCOMP_DATA_NR: u32 = 0;
            const SECCOMP_DATA_ARCH: u32 = 4;
            const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
            #[cfg(target_arch = "x86_64")]
            const AUDIT_ARCH: u32 = 0xc000_003e;
            #[cfg(target_arch = "aarch64")]
            const AUDIT_ARCH: u32 = 0xc000_00b7;
            let filters = vec![
                bpf(
                    libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
                    SECCOMP_DATA_ARCH,
                ),
                bpf_jump(
                    libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                    AUDIT_ARCH,
                    1,
                    0,
                ),
                bpf_return(SECCOMP_RET_KILL_PROCESS),
                bpf(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, SECCOMP_DATA_NR),
                bpf_jump(
                    libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                    SYS_SOCKET,
                    0,
                    1,
                ),
                bpf_return(libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
                bpf_return(libc::SECCOMP_RET_ALLOW),
            ];
            let seccomp_fd = test_seccomp_fd(&filters);
            let mut sockets = [-1_i32; 2];
            assert_eq!(
                unsafe {
                    libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sockets.as_mut_ptr())
                },
                0
            );
            let child_socket = sockets[1];
            let seccomp_arg = seccomp_fd.to_string();
            let mut command = Command::new(bubblewrap_executable());
            command
                .args([
                    "--ro-bind",
                    "/usr",
                    "/usr",
                    "--symlink",
                    "usr/bin",
                    "/bin",
                    "--ro-bind-try",
                    "/lib",
                    "/lib",
                    "--symlink",
                    "usr/lib64",
                    "/lib64",
                    "--proc",
                    "/proc",
                    "--dev",
                    "/dev",
                    "--unshare-user",
                    "--unshare-pid",
                    "--as-pid-1",
                    "--die-with-parent",
                    "--seccomp",
                    &seccomp_arg,
                    "--",
                    "/usr/bin/python3",
                    "-c",
                    concat!(
                        "import errno,os,socket,sys\n",
                        "s=socket.socket(fileno=3)\n",
                        "s.sendall(b'existing-socket-ok')\n",
                        "s.close()\n",
                        "try: socket.socket(socket.AF_INET,socket.SOCK_STREAM)\n",
                        "except OSError as error:\n",
                        " print('NEW_SOCKET_DENIED',error.errno,flush=True)\n",
                        " sys.exit(0 if error.errno == errno.EPERM else 97)\n",
                        "print('NEW_SOCKET_ALLOWED',flush=True)\n",
                        "sys.exit(98)\n",
                    ),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            unsafe {
                command.pre_exec(move || {
                    if child_socket != 3 {
                        if libc::dup2(child_socket, 3) < 0 {
                            return Err(io::Error::last_os_error());
                        }
                        libc::close(child_socket);
                    }
                    let flags = libc::fcntl(3, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let output = match command.output() {
                Ok(output) => output,
                Err(failure) => panic!("failed to run socket-denial probe: {failure}"),
            };
            unsafe {
                libc::close(seccomp_fd);
                libc::close(child_socket);
            }
            assert!(
                output.status.success(),
                "stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("NEW_SOCKET_DENIED"),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            let mut inherited = unsafe { std::fs::File::from_raw_fd(sockets[0]) };
            let mut received = String::new();
            inherited.read_to_string(&mut received).unwrap();
            assert_eq!(received, "existing-socket-ok");
        }
    }

    fn test_seccomp_fd(filters: &[libc::sock_filter]) -> RawFd {
        let mut pipe = [-1_i32; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        let bytes = filter_bytes(filters);
        let mut writer = unsafe { std::fs::File::from_raw_fd(pipe[1]) };
        writer.write_all(bytes).unwrap();
        writer.flush().unwrap();
        drop(writer);
        let inherited = unsafe { libc::fcntl(pipe[0], libc::F_DUPFD_CLOEXEC, 10) };
        unsafe {
            libc::close(pipe[0]);
        }
        assert!(inherited >= 0);
        let flags = unsafe { libc::fcntl(inherited, libc::F_GETFD) };
        assert!(flags >= 0);
        assert!(unsafe { libc::fcntl(inherited, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } >= 0);
        inherited
    }
}
