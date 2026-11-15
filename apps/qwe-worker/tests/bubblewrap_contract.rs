#![cfg(target_os = "linux")]

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const BWRAP_SHELL: &[&str] = &[
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
    "--unshare-pid",
    "--as-pid-1",
    "--die-with-parent",
    "--clearenv",
    "--setenv",
    "PATH",
    "/usr/bin:/bin",
];

fn bubblewrap_available() -> bool {
    let installed = Command::new("bwrap")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    installed
        && Command::new("bwrap")
            .args(BWRAP_SHELL)
            .arg("/bin/true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
}

fn skip_without_bubblewrap() -> bool {
    if bubblewrap_available() {
        false
    } else {
        eprintln!("SKIP: bwrap or unprivileged namespaces are unavailable");
        true
    }
}

fn pipe_above_ten() -> std::io::Result<(RawFd, RawFd)> {
    let mut descriptors = [-1_i32; 2];
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut duplicated = [-1_i32; 2];
    for (index, descriptor) in descriptors.into_iter().enumerate() {
        duplicated[index] = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 10) };
        unsafe {
            libc::close(descriptor);
        }
        if duplicated[index] < 0 {
            let failure = std::io::Error::last_os_error();
            for duplicated_descriptor in duplicated {
                if duplicated_descriptor >= 0 {
                    unsafe {
                        libc::close(duplicated_descriptor);
                    }
                }
            }
            return Err(failure);
        }
    }
    Ok((duplicated[0], duplicated[1]))
}

fn clear_close_on_exec(descriptor: RawFd) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[test]
fn bwrap_preserves_fd3_cgroup_and_reports_host_child_pid() {
    if skip_without_bubblewrap() {
        return;
    }
    let (credential_read, credential_write) = pipe_above_ten().unwrap();
    let (status_read, status_write) = pipe_above_ten().unwrap();
    let mut credential_writer = unsafe { File::from_raw_fd(credential_write) };
    credential_writer.write_all(b"qwe-secret-marker").unwrap();
    drop(credential_writer);

    let mut command = Command::new("bwrap");
    command
        .args(BWRAP_SHELL)
        .arg("--json-status-fd")
        .arg(status_write.to_string())
        .arg("/bin/sh")
        .arg("-c")
        .arg("read value <&3; printf 'fd3=%s cgroup=%s\\n' \"$value\" \"$(cat /proc/self/cgroup)\"; sleep 1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(move || {
            if credential_read != 3 {
                if libc::dup2(credential_read, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(credential_read);
            }
            clear_close_on_exec(3)?;
            clear_close_on_exec(status_write)?;
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    let outer_pid = child.id();
    unsafe {
        libc::close(credential_read);
        libc::close(status_write);
    }
    let mut status_reader = BufReader::new(unsafe { File::from_raw_fd(status_read) });
    let mut start_line = String::new();
    status_reader.read_line(&mut start_line).unwrap();
    let start: serde_json::Value = serde_json::from_str(&start_line).unwrap();
    let host_child_pid = start["child-pid"].as_u64().unwrap() as u32;
    eprintln!("bwrap outer pid={outer_pid}, json child-pid={host_child_pid}");

    let parent_cgroup = qwe_runtime::current_cgroup_path().unwrap();
    assert!(
        parent_cgroup.join("cgroup.kill").is_file(),
        "cgroup.kill is unavailable at {}",
        parent_cgroup.display()
    );
    let parent_relative = parent_cgroup
        .strip_prefix("/sys/fs/cgroup")
        .unwrap_or(&parent_cgroup);
    let child_cgroup = std::fs::read_to_string(format!("/proc/{host_child_pid}/cgroup"))
        .unwrap_or_else(|failure| panic!("child cgroup unavailable: {failure}"));
    assert!(
        child_cgroup.contains(parent_relative.to_string_lossy().as_ref()),
        "child cgroup {child_cgroup:?} did not inherit {}",
        parent_cgroup.display()
    );
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fd3=qwe-secret-marker"), "{stdout}");
}

#[test]
fn bwrap_pty_runs_without_new_session() {
    if skip_without_bubblewrap() {
        return;
    }
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut builder = CommandBuilder::new("bwrap");
    builder.args(BWRAP_SHELL);
    builder.arg("/bin/sh");
    builder.arg("-i");
    let mut child = pair.slave.spawn_command(builder).unwrap();
    drop(pair.slave);
    let child_pid = child.process_id().unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut output = Vec::new();
        let _ = reader.read_to_end(&mut output);
        let _ = sender.send(output);
    });
    thread::sleep(Duration::from_millis(200));
    let foreground = unsafe { libc::tcgetpgrp(pair.master.as_raw_fd().unwrap()) };
    assert!(foreground > 0, "PTY has no foreground process group");
    eprintln!("bwrap outer pid={child_pid}, pty foreground pgrp={foreground}");
    let mut writer = pair.master.take_writer().unwrap();
    writer
        .write_all(b"printf 'BWRAP_PTY_MARKER\\n'; exit 0\n")
        .unwrap();
    writer.flush().unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.exit_code(), 0);
    let output = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        output
            .windows(b"BWRAP_PTY_MARKER".len())
            .any(|window| window == b"BWRAP_PTY_MARKER"),
        "PTY output did not contain marker: {:?}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn bwrap_new_session_maps_pty_foreground_to_outer_pid() {
    if skip_without_bubblewrap() {
        return;
    }
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut builder = CommandBuilder::new("bwrap");
    builder.args(BWRAP_SHELL);
    builder.arg("--new-session");
    builder.args(["/bin/sh", "-i"]);
    let mut child = pair.slave.spawn_command(builder).unwrap();
    let child_pid = child.process_id().unwrap();
    thread::sleep(Duration::from_millis(200));
    let foreground = unsafe { libc::tcgetpgrp(pair.master.as_raw_fd().unwrap()) };
    assert!(
        foreground > 0,
        "--new-session left the PTY without a foreground group"
    );
    assert_eq!(
        foreground as u32, child_pid,
        "--new-session did not map the PTY foreground group to the outer bwrap PID"
    );
    eprintln!(
        "bwrap --new-session outer pid={}, pty foreground pgrp={foreground}",
        child_pid
    );
    let mut writer = pair.master.take_writer().unwrap();
    writer.write_all(b"exit 0\n").unwrap();
    writer.flush().unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.exit_code(), 0);
}

#[test]
fn bwrap_pty_preserves_resize_signals_and_job_control_without_new_session() {
    if skip_without_bubblewrap() {
        return;
    }
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut builder = CommandBuilder::new("bwrap");
    builder.args(BWRAP_SHELL);
    builder.args(["/bin/bash", "--noprofile", "--norc", "-i"]);
    let mut child = pair.slave.spawn_command(builder).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let mut writer = pair.master.take_writer().unwrap();
    let mut output = Vec::new();
    thread::sleep(Duration::from_millis(200));

    writer
        .write_all(b"stty size; printf '\\nSIZE_%s\\n' FIRST_DONE\n")
        .unwrap();
    read_until(
        &receiver,
        &mut output,
        b"SIZE_FIRST_DONE",
        Duration::from_secs(2),
    );
    assert!(
        output
            .windows(b"24 80".len())
            .any(|window| window == b"24 80"),
        "initial PTY size not visible: {:?}",
        String::from_utf8_lossy(&output)
    );

    pair.master
        .resize(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    writer
        .write_all(b"stty size; printf '\\nSIZE_%s\\n' SECOND_DONE\n")
        .unwrap();
    read_until(
        &receiver,
        &mut output,
        b"SIZE_SECOND_DONE",
        Duration::from_secs(2),
    );
    assert!(
        output
            .windows(b"40 120".len())
            .any(|window| window == b"40 120"),
        "resized PTY dimensions not visible: {:?}",
        String::from_utf8_lossy(&output)
    );

    writer.write_all(b"sleep 30\n").unwrap();
    thread::sleep(Duration::from_millis(200));
    writer.write_all(b"\x03").unwrap();
    writer
        .write_all(b"printf 'ALIVE_AFTER_%s\\n' INT\n")
        .unwrap();
    read_until(
        &receiver,
        &mut output,
        b"ALIVE_AFTER_INT",
        Duration::from_secs(2),
    );

    writer.write_all(b"sleep 30\n").unwrap();
    thread::sleep(Duration::from_millis(200));
    writer.write_all(b"\x1a").unwrap();
    writer
        .write_all(b"jobs; printf 'ALIVE_AFTER_%s\\n' TSTP\n")
        .unwrap();
    read_until(
        &receiver,
        &mut output,
        b"ALIVE_AFTER_TSTP",
        Duration::from_secs(2),
    );
    assert!(
        output
            .windows(b"Stopped".len())
            .any(|window| window == b"Stopped"),
        "shell job control did not stop the foreground job: {:?}",
        String::from_utf8_lossy(&output)
    );

    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn bwrap_pty_master_resize_delivers_sigwinch_without_shim() {
    if skip_without_bubblewrap() {
        return;
    }
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut builder = CommandBuilder::new("bwrap");
    builder.args(BWRAP_SHELL);
    builder.args(["/bin/bash", "--noprofile", "--norc", "-i"]);
    let mut child = pair.slave.spawn_command(builder).unwrap();
    drop(pair.slave);
    let outer_pid = child.process_id().unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let mut writer = pair.master.take_writer().unwrap();
    let mut output = Vec::new();
    writer
        .write_all(b"trap 'printf \"\\nWINCH_SEEN\\n\"' WINCH; printf 'RESIZE_READY\\n'\n")
        .unwrap();
    writer.flush().unwrap();
    read_until(
        &receiver,
        &mut output,
        b"RESIZE_READY",
        Duration::from_secs(2),
    );
    let foreground_before = unsafe { libc::tcgetpgrp(pair.master.as_raw_fd().unwrap()) };
    assert!(foreground_before > 0, "PTY has no foreground process group");
    pair.master
        .resize(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    read_until(
        &receiver,
        &mut output,
        b"WINCH_SEEN",
        Duration::from_secs(2),
    );
    let foreground_after = unsafe { libc::tcgetpgrp(pair.master.as_raw_fd().unwrap()) };
    eprintln!(
        "PTY resize outer pid={outer_pid} foreground_before={foreground_before} foreground_after={foreground_after}"
    );
    assert_eq!(foreground_before, foreground_after);
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn bwrap_pid1_ignores_default_sigterm_without_a_handler() {
    if skip_without_bubblewrap() {
        return;
    }
    let (status_read, status_write) = pipe_above_ten().unwrap();
    let mut command = Command::new("bwrap");
    command
        .args(BWRAP_SHELL)
        .arg("--json-status-fd")
        .arg(status_write.to_string())
        .args([
            "/usr/bin/python3",
            "-c",
            "import time; print('PID1_READY', flush=True); time.sleep(60)",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(move || clear_close_on_exec(status_write));
    }
    let mut child = command.spawn().unwrap();
    unsafe {
        libc::close(status_write);
    }
    let (child_pid, _status_reader) = read_bwrap_child_pid(status_read);
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains("PID1_READY"), "{line:?}");
    assert_eq!(unsafe { libc::kill(child_pid, libc::SIGTERM) }, 0);
    thread::sleep(Duration::from_millis(250));
    assert_eq!(
        unsafe { libc::kill(child_pid, 0) },
        0,
        "PID 1 exited on default SIGTERM behavior"
    );
    unsafe {
        libc::kill(child_pid, libc::SIGKILL);
    }
    let _ = child.wait();
}

#[test]
fn bwrap_pid1_explicit_signal_and_reaper_handlers_work() {
    if skip_without_bubblewrap() {
        return;
    }
    let script = concat!(
        "import os,signal,sys,time\n",
        "reaped=[]\n",
        "def on_chld(sig,frame):\n",
        " while True:\n",
        "  try: pid,status=os.waitpid(-1,os.WNOHANG)\n",
        "  except ChildProcessError: return\n",
        "  if pid == 0: return\n",
        "  reaped.append(pid); print('PID1_REAPED',flush=True)\n",
        "def on_term(sig,frame): print('PID1_TERM',flush=True)\n",
        "def on_int(sig,frame): print('PID1_INT',flush=True); sys.exit(0)\n",
        "signal.signal(signal.SIGCHLD,on_chld)\n",
        "signal.signal(signal.SIGTERM,on_term)\n",
        "signal.signal(signal.SIGINT,on_int)\n",
        "pid=os.fork()\n",
        "if pid == 0: os._exit(7)\n",
        "print('PID1_READY',flush=True)\n",
        "while not reaped: time.sleep(0.05)\n",
        "while True: time.sleep(1)\n",
    );
    let (status_read, status_write) = pipe_above_ten().unwrap();
    let mut command = Command::new("bwrap");
    command
        .args(BWRAP_SHELL)
        .arg("--json-status-fd")
        .arg(status_write.to_string())
        .args(["/usr/bin/python3", "-c", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(move || clear_close_on_exec(status_write));
    }
    let mut child = command.spawn().unwrap();
    unsafe {
        libc::close(status_write);
    }
    let (child_pid, _status_reader) = read_bwrap_child_pid(status_read);
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut reader = stdout;
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let mut output = Vec::new();
    read_until(
        &receiver,
        &mut output,
        b"PID1_REAPED",
        Duration::from_secs(2),
    );
    assert_eq!(unsafe { libc::kill(child_pid, libc::SIGTERM) }, 0);
    read_until(&receiver, &mut output, b"PID1_TERM", Duration::from_secs(2));
    assert_eq!(unsafe { libc::kill(child_pid, libc::SIGINT) }, 0);
    read_until(&receiver, &mut output, b"PID1_INT", Duration::from_secs(2));
    let status = child.wait().unwrap();
    assert!(
        status.success() || status.signal() == Some(libc::SIGINT),
        "unexpected PID-1 terminal status: {status:?}"
    );
}

fn read_bwrap_child_pid(status_read: RawFd) -> (i32, BufReader<File>) {
    let mut status_reader = BufReader::new(unsafe { File::from_raw_fd(status_read) });
    let mut start_line = String::new();
    status_reader.read_line(&mut start_line).unwrap();
    let start: serde_json::Value = serde_json::from_str(&start_line).unwrap();
    (start["child-pid"].as_u64().unwrap() as i32, status_reader)
}

fn read_until(
    receiver: &std::sync::mpsc::Receiver<Vec<u8>>,
    output: &mut Vec<u8>,
    marker: &[u8],
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        if output.windows(marker.len()).any(|window| window == marker) {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for {marker:?}");
        let chunk = receiver
            .recv_timeout(remaining)
            .unwrap_or_else(|_| panic!("timed out waiting for {marker:?}"));
        output.extend_from_slice(&chunk);
    }
}

#[test]
fn bwrap_die_with_parent_survives_spawning_thread_exit() {
    if skip_without_bubblewrap() {
        return;
    }
    let (status_read, status_write) = pipe_above_ten().unwrap();
    let (child_sender, child_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    let thread = thread::spawn(move || {
        let mut command = Command::new("bwrap");
        command
            .args(BWRAP_SHELL)
            .arg("--json-status-fd")
            .arg(status_write.to_string())
            .args(["/bin/sleep", "5"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(move || clear_close_on_exec(status_write));
        }
        let child = command.spawn().unwrap();
        let child_id = child.id();
        child_sender.send(child).unwrap();
        unsafe {
            libc::close(status_write);
        }
        release_receiver.recv().unwrap();
        child_id
    });
    let mut child = child_receiver.recv().unwrap();
    let child_pid = child.id();
    let mut status_reader = BufReader::new(unsafe { File::from_raw_fd(status_read) });
    let mut start_line = String::new();
    status_reader.read_line(&mut start_line).unwrap();
    let start: serde_json::Value = serde_json::from_str(&start_line).unwrap();
    assert!(start["child-pid"].as_u64().is_some(), "{start}");
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, child_pid, 0) };
    assert!(
        pidfd >= 0,
        "pidfd_open failed: {}",
        std::io::Error::last_os_error()
    );
    release_sender.send(()).unwrap();
    assert_eq!(thread.join().unwrap(), child_pid);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if child.try_wait().unwrap().is_some() {
            unsafe {
                libc::close(pidfd as i32);
            }
            return;
        }
        if Instant::now() >= deadline {
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    pidfd,
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
                libc::close(pidfd as i32);
            }
            let _ = child.kill();
            let _ = child.wait();
            panic!("bwrap child survived its spawning thread");
        }
        thread::sleep(Duration::from_millis(20));
    }
}
