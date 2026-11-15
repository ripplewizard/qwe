use clap::Parser;
use serde::Deserialize;
use std::collections::HashSet;
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::io::FromRawFd;
use std::os::unix::net::UnixStream;

#[derive(Debug, Parser)]
#[command(version, about = "Internal QWE Session resource-limit launcher")]
struct Args {
    #[arg(long, hide = true)]
    release_fd: Option<i32>,
    #[arg(long)]
    topology_socket: Option<std::path::PathBuf>,
    #[arg(long)]
    credential_socket: Option<std::path::PathBuf>,
    #[arg(long, hide = true)]
    credential_fd: Option<i32>,
    #[arg(long, hide = true)]
    sandbox_status_socket: Option<std::path::PathBuf>,
    #[arg(long, hide = true)]
    sandbox_status_fd: Option<i32>,
    #[arg(long, hide = true)]
    sandbox_seccomp_file: Option<std::path::PathBuf>,
    #[arg(long, hide = true)]
    sandbox_seccomp_fd: Option<i32>,
    #[arg(long, hide = true)]
    control_socket: Option<std::path::PathBuf>,
    #[arg(long, hide = true)]
    control_fd: Option<i32>,
    #[arg(long, hide = true)]
    pane_mount_manifest: Option<std::path::PathBuf>,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PaneMountManifest {
    mounts: Vec<PaneMount>,
}

#[derive(Debug, Deserialize)]
struct PaneMount {
    fd: i32,
    path: std::path::PathBuf,
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(descriptor) = args.release_fd {
        let mut release = unsafe { File::from_raw_fd(descriptor) };
        let mut acknowledged = [0_u8; 1];
        release.read_exact(&mut acknowledged)?;
        if acknowledged != *b"1" {
            return Err("agent process release handshake was rejected".into());
        }
    }
    if args.credential_socket.is_some() && args.credential_fd.is_some() {
        return Err("credential socket and descriptor are mutually exclusive".into());
    }
    if let Some(path) = &args.credential_socket {
        let stream = UnixStream::connect(path)
            .map_err(|failure| format!("failed to connect credential broker: {failure}"))?;
        map_credential_descriptor(stream.as_raw_fd())?;
        std::mem::forget(stream);
    } else if let Some(source) = args.credential_fd {
        map_credential_descriptor(source)?;
    }
    match (&args.sandbox_seccomp_file, args.sandbox_seccomp_fd) {
        (Some(path), Some(target)) => {
            let file = File::open(path)
                .map_err(|failure| format!("failed to open sandbox seccomp file: {failure}"))?;
            map_descriptor(file.as_raw_fd(), target, "sandbox seccomp")?;
            std::mem::forget(file);
            std::fs::remove_file(path)
                .map_err(|failure| format!("failed to remove sandbox seccomp file: {failure}"))?;
        }
        (None, None) => {}
        _ => {
            return Err("sandbox seccomp file and descriptor must be provided together".into());
        }
    }
    match (&args.sandbox_status_socket, args.sandbox_status_fd) {
        (Some(path), Some(target)) => {
            let stream = UnixStream::connect(path)
                .map_err(|failure| format!("failed to connect sandbox status socket: {failure}"))?;
            map_descriptor(stream.as_raw_fd(), target, "sandbox status")?;
            std::mem::forget(stream);
        }
        (None, None) => {}
        _ => {
            return Err("sandbox status socket and descriptor must be provided together".into());
        }
    }
    match (&args.control_socket, args.control_fd) {
        (Some(path), Some(target)) => {
            let stream = UnixStream::connect(path)
                .map_err(|failure| format!("failed to connect Pane control broker: {failure}"))?;
            qwe_protocol::verify_same_uid(&stream)
                .map_err(|failure| format!("Pane control broker identity rejected: {failure}"))?;
            map_descriptor(stream.as_raw_fd(), target, "Pane control")?;
            std::mem::forget(stream);
        }
        (None, None) => {}
        _ => {
            return Err("Pane control socket and descriptor must be provided together".into());
        }
    }
    if let Some(topology_socket) = &args.topology_socket {
        let cgroup = qwe_runtime::current_cgroup_path()
            .map_err(|failure| format!("failed to resolve Session cgroup: {failure}"))?;
        let payload = serde_json::json!({
            "pid": std::process::id(),
            "cgroup": cgroup
        });
        let mut stream = UnixStream::connect(topology_socket)
            .map_err(|failure| format!("failed to connect topology broker: {failure}"))?;
        serde_json::to_writer(&mut stream, &payload)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        let mut acknowledgement = [0_u8; 1];
        stream.read_exact(&mut acknowledgement)?;
        if acknowledgement != *b"1" {
            return Err("resource topology broker rejected the handshake".into());
        }
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    if let Some(path) = &args.pane_mount_manifest {
        install_pane_mounts(path)?;
    }
    let program = CString::new(args.command[0].as_bytes())?;
    let arguments = args
        .command
        .iter()
        .map(|value| CString::new(value.as_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut pointers = arguments
        .iter()
        .map(|value| value.as_ptr())
        .collect::<Vec<_>>();
    pointers.push(std::ptr::null());
    unsafe {
        libc::execvp(program.as_ptr(), pointers.as_ptr());
    }
    let failure = std::io::Error::last_os_error();
    Err(format!("failed to exec {}: {}", args.command[0], failure).into())
}

fn install_pane_mounts(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let result = (|| {
        let manifest = serde_json::from_slice::<PaneMountManifest>(&std::fs::read(path)?).map_err(
            |failure| {
                format!(
                    "failed to parse Pane mount manifest '{}': {failure}",
                    path.display()
                )
            },
        )?;
        let mut targets = HashSet::new();
        for mount in manifest.mounts {
            if !targets.insert(mount.fd) {
                return Err(format!("Pane mount target {} is duplicated", mount.fd).into());
            }
            let source = CString::new(mount.path.as_os_str().as_bytes()).map_err(|_| {
                format!(
                    "Pane mount path '{}' contains an embedded NUL",
                    mount.path.display()
                )
            })?;
            let descriptor = unsafe { libc::open(source.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
            if descriptor < 0 {
                return Err(format!(
                    "failed to pin Pane mount '{}': {}",
                    mount.path.display(),
                    std::io::Error::last_os_error()
                )
                .into());
            }
            if let Err(failure) = map_descriptor(descriptor, mount.fd, "Pane mount") {
                unsafe {
                    libc::close(descriptor);
                }
                return Err(failure);
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(path);
    result
}

fn map_credential_descriptor(source: i32) -> Result<(), Box<dyn std::error::Error>> {
    map_descriptor(source, 3, "credential")?;
    unsafe {
        std::env::set_var("QWE_CREDENTIAL_FD", "3");
    }
    Ok(())
}

fn map_descriptor(
    source: i32,
    target: i32,
    description: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if target < 3 {
        return Err(format!("{description} target descriptor must be at least 3").into());
    }
    if source != target && unsafe { libc::dup2(source, target) } < 0 {
        return Err(format!(
            "failed to map {description} descriptor: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    if source != target {
        unsafe {
            libc::close(source);
        }
    }
    if unsafe { libc::fcntl(target, libc::F_SETFD, 0) } < 0 {
        return Err(format!(
            "failed to make {description} descriptor inheritable: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    let flags = unsafe { libc::fcntl(target, libc::F_GETFD) };
    if flags < 0 || flags & libc::FD_CLOEXEC != 0 {
        return Err(format!("{description} descriptor remains close-on-exec").into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(Args::parse())
}
