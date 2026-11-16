use clap::{Parser, ValueEnum};
#[cfg(target_os = "linux")]
use qwe_core::SANDBOXED_PANE_SHUTDOWN_GRACE_MS;
use qwe_runtime::{LandlockPolicy, apply_landlock};
#[cfg(target_os = "linux")]
use qwe_runtime::{PaneControlDecoder, PaneControlMessage};
use std::ffi::CString;
#[cfg(target_os = "linux")]
use std::io;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicI32, Ordering};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WorkspaceMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Parser)]
#[command(
    name = "qwe-sandbox-init",
    version,
    about = "Internal QWE Landlock sandbox initializer"
)]
struct Args {
    #[arg(long)]
    workspace: PathBuf,
    #[arg(long, value_enum)]
    workspace_mode: WorkspaceMode,
    #[arg(long)]
    private_tmp: bool,
    #[arg(long, hide = true)]
    pane_init: bool,
    #[arg(long, hide = true)]
    control_fd: Option<i32>,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

fn run(args: Args) -> Result<i32, Box<dyn std::error::Error>> {
    let abi = apply_landlock(&LandlockPolicy {
        workspace: &args.workspace,
        workspace_read_write: matches!(args.workspace_mode, WorkspaceMode::ReadWrite),
        private_tmp: args.private_tmp,
    })?;
    harden_process()?;
    qwe_runtime::check_sandbox_fault("after_dumpability")?;
    unsafe {
        std::env::set_var("QWE_LANDLOCK_ABI", abi.to_string());
    }
    if args.pane_init {
        return run_pane_init(&args);
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
    qwe_runtime::check_sandbox_fault("before_exec")?;
    unsafe {
        libc::execvp(program.as_ptr(), pointers.as_ptr());
    }
    let failure = std::io::Error::last_os_error();
    Err(format!("failed to exec {}: {}", args.command[0], failure).into())
}

#[cfg(target_os = "linux")]
static SIGNAL_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

#[cfg(target_os = "linux")]
static PENDING_SIGNALS: AtomicI32 = AtomicI32::new(0);

#[cfg(target_os = "linux")]
fn signal_mask(signal: libc::c_int) -> i32 {
    1_i32 << (signal - 1)
}

#[cfg(target_os = "linux")]
extern "C" fn signal_handler(signal: libc::c_int) {
    let saved_errno = unsafe { *libc::__errno_location() };
    PENDING_SIGNALS.fetch_or(signal_mask(signal), Ordering::Relaxed);
    let descriptor = SIGNAL_WRITE_FD.load(Ordering::Relaxed);
    if descriptor >= 0 {
        let byte = signal as u8;
        unsafe {
            libc::write(descriptor, (&byte as *const u8).cast(), 1);
        }
    }
    unsafe {
        *libc::__errno_location() = saved_errno;
    }
}

#[cfg(target_os = "linux")]
fn run_pane_init(args: &Args) -> Result<i32, Box<dyn std::error::Error>> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::time::{Duration, Instant};

    ensure_controlling_terminal()?;

    let (read_fd, write_fd) = create_signal_pipe()?;
    let control_fd = args
        .control_fd
        .map(prepare_control_fd)
        .transpose()?
        .unwrap_or(-1);
    let mut control_decoder = PaneControlDecoder::default();
    PENDING_SIGNALS.store(0, Ordering::Relaxed);
    SIGNAL_WRITE_FD.store(write_fd, Ordering::Relaxed);
    for signal in [libc::SIGCHLD, libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        install_signal_handler(signal)?;
    }

    let mut child = std::process::Command::new(&args.command[0])
        .args(&args.command[1..])
        .process_group(0)
        .spawn()
        .map_err(|failure| format!("pane init failed to start shell: {failure}"))?;
    let shell_pid = child.id() as libc::pid_t;
    if unsafe { libc::tcsetpgrp(0, shell_pid) } < 0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "pane init failed to assign foreground process group: {}",
            io::Error::last_os_error()
        )
        .into());
    }

    let mut shell_status = None;
    let mut cleanup_deadline = None;
    let mut kill_deadline = None;
    let mut killed = false;
    loop {
        let pending = PENDING_SIGNALS.swap(0, Ordering::Relaxed);
        if pending & signal_mask(libc::SIGCHLD) != 0 {
            drain_signals(&mut shell_status, shell_pid)?;
        }
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            if pending & signal_mask(signal) != 0 {
                forward_signal(signal, shell_pid)?;
            }
        }
        if control_fd >= 0 {
            let messages = read_control_messages(control_fd, &mut control_decoder)?;
            for message in messages {
                match message {
                    PaneControlMessage::Signal(signal) => forward_signal(signal, shell_pid)?,
                }
            }
        }
        if shell_status.is_some() && cleanup_deadline.is_none() {
            signal_pane_tree(shell_pid, libc::SIGTERM);
            cleanup_deadline =
                Some(Instant::now() + Duration::from_millis(SANDBOXED_PANE_SHUTDOWN_GRACE_MS));
        }
        if cleanup_deadline.is_some_and(|deadline| Instant::now() >= deadline) && !killed {
            signal_pane_tree(shell_pid, libc::SIGKILL);
            killed = true;
            kill_deadline = Some(Instant::now() + Duration::from_secs(1));
        }
        if kill_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }

        let mut descriptors = [
            libc::pollfd {
                fd: read_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: control_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let descriptor_count = if control_fd >= 0 { 2 } else { 1 };
        let ready = unsafe { libc::poll(descriptors.as_mut_ptr(), descriptor_count, 100) };
        if ready < 0 {
            let failure = io::Error::last_os_error();
            if failure.raw_os_error() != Some(libc::EINTR) {
                return Err(format!("pane init poll failed: {failure}").into());
            }
        }
        if descriptors[0].revents & libc::POLLIN != 0 {
            drain_signal_pipe(read_fd)?;
        }
        if descriptors[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let messages = read_control_messages(control_fd, &mut control_decoder)?;
            for message in messages {
                match message {
                    PaneControlMessage::Signal(signal) => forward_signal(signal, shell_pid)?,
                }
            }
        }
    }
    unsafe {
        libc::close(read_fd);
        libc::close(write_fd);
        if control_fd >= 0 {
            libc::close(control_fd);
        }
    }
    SIGNAL_WRITE_FD.store(-1, Ordering::Relaxed);
    PENDING_SIGNALS.store(0, Ordering::Relaxed);
    let status = shell_status.ok_or("pane init lost shell exit status")?;
    let status: std::process::ExitStatus = std::os::unix::process::ExitStatusExt::from_raw(status);
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
}

#[cfg(target_os = "linux")]
fn signal_pane_tree(shell_pid: libc::pid_t, signal: libc::c_int) {
    let target = if unsafe { libc::getpid() } == 1 {
        -1
    } else {
        -shell_pid
    };
    unsafe {
        libc::kill(target, signal);
    }
}

#[cfg(target_os = "linux")]
fn prepare_control_fd(descriptor: i32) -> io::Result<i32> {
    if descriptor < 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "control descriptor must be at least 3",
        ));
    }
    let status_flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if status_flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(descriptor, libc::F_SETFD, status_flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let descriptor_flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if descriptor_flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe {
        libc::fcntl(
            descriptor,
            libc::F_SETFL,
            descriptor_flags | libc::O_NONBLOCK,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(descriptor)
}

#[cfg(target_os = "linux")]
fn read_control_messages(
    descriptor: i32,
    decoder: &mut PaneControlDecoder,
) -> io::Result<Vec<PaneControlMessage>> {
    let mut buffer = [0_u8; 128];
    loop {
        let count = unsafe { libc::read(descriptor, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count > 0 {
            let count = usize::try_from(count).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "negative control read length")
            })?;
            return decoder.push(&buffer[..count]).map_err(|failure| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("control error: {failure}"),
                )
            });
        }
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "control channel closed",
            ));
        }
        let failure = io::Error::last_os_error();
        match failure.raw_os_error() {
            Some(libc::EAGAIN) => return Ok(Vec::new()),
            Some(libc::EINTR) => continue,
            _ => return Err(failure),
        }
    }
}

#[cfg(target_os = "linux")]
fn ensure_controlling_terminal() -> Result<(), Box<dyn std::error::Error>> {
    if unsafe { libc::tcgetsid(0) } >= 0 {
        return Ok(());
    }
    if unsafe { libc::setsid() } < 0 {
        return Err(format!(
            "pane init failed to create a session: {}",
            io::Error::last_os_error()
        )
        .into());
    }
    if unsafe { libc::ioctl(0, libc::TIOCSCTTY, 0) } < 0 {
        return Err(format!(
            "pane init failed to acquire controlling terminal: {}",
            io::Error::last_os_error()
        )
        .into());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run_pane_init(_args: &Args) -> Result<i32, Box<dyn std::error::Error>> {
    Err("pane-init mode is supported only on Linux".into())
}

#[cfg(target_os = "linux")]
fn create_signal_pipe() -> io::Result<(i32, i32)> {
    let mut descriptors = [-1_i32; 2];
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((descriptors[0], descriptors[1]))
}

#[cfg(target_os = "linux")]
fn install_signal_handler(signal: libc::c_int) -> io::Result<()> {
    let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
    action.sa_sigaction = signal_handler as *const () as usize;
    action.sa_flags = libc::SA_RESTART;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
    }
    if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn drain_signal_pipe(read_fd: i32) -> io::Result<()> {
    let mut buffer = [0_u8; 64];
    loop {
        let count = unsafe { libc::read(read_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count > 0 {
            continue;
        }
        if count == 0 {
            return Ok(());
        }
        let failure = io::Error::last_os_error();
        match failure.raw_os_error() {
            Some(libc::EAGAIN) => return Ok(()),
            Some(libc::EINTR) => continue,
            _ => return Err(failure),
        }
    }
}

#[cfg(target_os = "linux")]
fn forward_signal(signal: libc::c_int, shell_pid: libc::pid_t) -> io::Result<()> {
    let foreground = unsafe { libc::tcgetpgrp(0) };
    let pgid = if foreground > 0 {
        foreground
    } else {
        shell_pid
    };
    if unsafe { libc::kill(-pgid, signal) } < 0 {
        let failure = io::Error::last_os_error();
        if failure.raw_os_error() != Some(libc::ESRCH) {
            return Err(failure);
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn drain_signals(shell_status: &mut Option<i32>, shell_pid: libc::pid_t) -> io::Result<()> {
    loop {
        let mut raw_status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut raw_status, libc::WNOHANG) };
        if pid > 0 {
            if pid == shell_pid {
                *shell_status = Some(raw_status);
            }
            continue;
        }
        if pid == 0 {
            return Ok(());
        }
        let failure = io::Error::last_os_error();
        if failure.raw_os_error() == Some(libc::ECHILD) {
            return Ok(());
        }
        return Err(failure);
    }
}

#[cfg(target_os = "linux")]
fn harden_process() -> Result<(), Box<dyn std::error::Error>> {
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } < 0 {
        return Err(format!(
            "failed to disable process dumpability: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn harden_process() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

fn main() {
    match run(Args::parse()) {
        Ok(code) => std::process::exit(code),
        Err(failure) => {
            eprintln!("qwe-sandbox-init: {failure}");
            std::process::exit(125);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(arguments)
    }

    #[test]
    fn parser_preserves_target_arguments_after_double_dash() {
        let args = parse(&[
            "qwe-sandbox-init",
            "--workspace",
            "/tmp",
            "--workspace-mode",
            "read-only",
            "--private-tmp",
            "--",
            "/bin/sh",
            "-c",
            "printf ok",
            "--target-option",
        ])
        .unwrap();
        assert_eq!(
            args.command,
            ["/bin/sh", "-c", "printf ok", "--target-option"]
        );
    }

    #[test]
    fn parser_rejects_missing_and_invalid_options() {
        assert!(
            parse(&[
                "qwe-sandbox-init",
                "--workspace-mode",
                "read-only",
                "--",
                "/bin/true",
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "qwe-sandbox-init",
                "--workspace",
                "/tmp",
                "--workspace-mode",
                "invalid",
                "--",
                "/bin/true",
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "qwe-sandbox-init",
                "--workspace",
                "/tmp",
                "--workspace-mode",
                "read-only",
                "--control-fd",
                "not-a-number",
                "--",
                "/bin/true",
            ])
            .is_err()
        );
    }
}
