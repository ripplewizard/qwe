use clap::Parser;
use std::ffi::CString;
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

#[derive(Debug, Parser)]
#[command(about = "Internal QWE Session resource-limit launcher")]
struct Args {
    #[arg(long)]
    topology_socket: Option<std::path::PathBuf>,
    #[arg(long)]
    credential_socket: Option<std::path::PathBuf>,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
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
        stream.flush()?;
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    if let Some(path) = args.credential_socket {
        let stream = UnixStream::connect(&path)
            .map_err(|failure| format!("failed to connect credential broker: {failure}"))?;
        let source = stream.as_raw_fd();
        if source != 3 && unsafe { libc::dup2(source, 3) } < 0 {
            return Err(format!(
                "failed to map credential descriptor: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        if source != 3 {
            unsafe {
                libc::close(source);
            }
        }
        if unsafe { libc::fcntl(3, libc::F_SETFD, 0) } < 0 {
            return Err(format!(
                "failed to make credential descriptor inheritable: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        let flags = unsafe { libc::fcntl(3, libc::F_GETFD) };
        if flags < 0 || flags & libc::FD_CLOEXEC != 0 {
            return Err("credential descriptor remains close-on-exec".into());
        }
        unsafe {
            std::env::set_var("QWE_CREDENTIAL_FD", "3");
        }
        std::mem::forget(stream);
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(Args::parse())
}
