use portable_pty::{native_pty_system, CommandBuilder, ExitStatus, PtySize};
use std::fs;
use std::io::{Read, Write};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

struct PtyRun {
    output: Vec<u8>,
    status: ExitStatus,
}

enum ReaderEvent {
    Data(Vec<u8>),
    End,
    Error(String),
}

fn spawn_reader(mut reader: Box<dyn Read + Send>) -> Receiver<ReaderEvent> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(ReaderEvent::End);
                    break;
                }
                Ok(count) => {
                    if sender
                        .send(ReaderEvent::Data(buffer[..count].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(ReaderEvent::Error(error.to_string()));
                    break;
                }
            }
        }
    });
    receiver
}

fn read_until(
    receiver: &Receiver<ReaderEvent>,
    output: &mut Vec<u8>,
    marker: &str,
) -> Result<bool, String> {
    let search_start = output.len();
    loop {
        if String::from_utf8_lossy(&output[search_start..]).contains(marker) {
            return Ok(true);
        }
        match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(ReaderEvent::Data(data)) => output.extend_from_slice(&data),
            Ok(ReaderEvent::End) => return Ok(false),
            Ok(ReaderEvent::Error(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => return Err("PTY read timed out".into()),
            Err(RecvTimeoutError::Disconnected) => {
                return Err("PTY reader disconnected".into());
            }
        }
    }
}

fn read_to_end(receiver: &Receiver<ReaderEvent>, output: &mut Vec<u8>) -> Result<(), String> {
    loop {
        match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(ReaderEvent::Data(data)) => output.extend_from_slice(&data),
            Ok(ReaderEvent::End) => return Ok(()),
            Ok(ReaderEvent::Error(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => return Err("PTY read timed out".into()),
            Err(RecvTimeoutError::Disconnected) => {
                return Err("PTY reader disconnected".into());
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn notify_foreground_resize(process_id: u32) -> Result<(), String> {
    let result = unsafe { libc::kill(-(process_id as libc::pid_t), libc::SIGWINCH) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

#[cfg(not(target_os = "macos"))]
fn notify_foreground_resize(_process_id: u32) -> Result<(), String> {
    Ok(())
}

fn run_pty_session(
    input: &str,
    log_path: Option<&str>,
) -> Result<PtyRun, Box<dyn std::error::Error>> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut command = CommandBuilder::new("sh");
    command.env("TERM", "xterm-256color");
    command.env("NVIM_TEST_MARKER", "QWE_NVIM_OK");
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    writer.write_all(input.as_bytes())?;
    writer.flush()?;
    drop(writer);

    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    let status = child.wait()?;

    if let Some(path) = log_path {
        fs::write(path, &output)?;
    }

    Ok(PtyRun { output, status })
}

fn run_pty_smoke(log_path: Option<&str>) -> Result<bool, Box<dyn std::error::Error>> {
    let run = run_pty_session(
        "printf 'QWE_PTY_READY\\n'; printf 'QWE_PTY_SIZE '; stty size; printf 'QWE_PTY_END\\n'; exit 0\r",
        log_path,
    )?;
    let text = String::from_utf8_lossy(&run.output);
    let shell = text.contains("QWE_PTY_READY") && text.contains("QWE_PTY_END");
    let window_size = text.contains("QWE_PTY_SIZE 24 80");
    let passed = run.status.success() && shell && window_size;

    println!(
        r#"{{"contract":"qwe-poc/0.1","feature":"pty-smoke","implementation":"rust","exit_code":{},"bytes":{},"checks":{{"shell":{},"window_size":{}}}}}"#,
        run.status.exit_code(),
        run.output.len(),
        shell,
        window_size
    );
    Ok(passed)
}

fn run_editor_smoke(log_path: Option<&str>) -> Result<bool, Box<dyn std::error::Error>> {
    let input = concat!(
        "printf 'QWE_EDITOR_SIZE '; stty size; ",
        "nvim --clean -u NONE -c 'call setline(1, $NVIM_TEST_MARKER)' ",
        "-c 'redraw' -c 'sleep 200m' -c 'qa!'; exit $?\r"
    );
    let run = run_pty_session(input, log_path)?;
    let text = String::from_utf8_lossy(&run.output);
    // The marker comes from the child environment, not the echoed command.
    let screen_text = text.contains("QWE_NVIM_OK");
    let window_size = text.contains("QWE_EDITOR_SIZE 24 80");
    let passed = run.status.success() && screen_text && window_size;

    println!(
        r#"{{"contract":"qwe-poc/0.1","feature":"editor-smoke","implementation":"rust","exit_code":{},"bytes":{},"checks":{{"process":{},"screen_text":{},"window_size":{}}}}}"#,
        run.status.exit_code(),
        run.output.len(),
        run.status.success(),
        screen_text,
        window_size
    );
    Ok(passed)
}

fn run_resize_smoke(log_path: Option<&str>) -> Result<bool, Box<dyn std::error::Error>> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut command = CommandBuilder::new("sh");
    command.env("TERM", "xterm-256color");
    command.env("QWE_READY_MARKER", "QWE_RESIZE_READY");
    command.env("QWE_WINCH_MARKER", "QWE_WINCH");
    command.env("QWE_SIZE_MARKER", "QWE_SIZE");
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);

    let reader = spawn_reader(pair.master.try_clone_reader()?);
    let mut writer = pair.master.take_writer()?;
    writer.write_all(
        b"trap 'printf \"%s \" \"$QWE_WINCH_MARKER\"; stty size' WINCH; printf '%s ' \"$QWE_READY_MARKER\"; stty size; while IFS= read -r line; do [ \"$line\" = exit ] && break; printf '%s ' \"$QWE_SIZE_MARKER\"; stty size; done; exit 0\r",
    )?;
    writer.flush()?;

    let process_id = child
        .process_id()
        .ok_or("child process does not expose a process id")?;
    let mut output = Vec::new();
    let mut checks = Vec::new();
    let ready = read_until(&reader, &mut output, "QWE_RESIZE_READY 24 80")?;

    let sizes = [(120, 40), (80, 24), (160, 50), (40, 15)];
    for (cols, rows) in sizes {
        pair.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        notify_foreground_resize(process_id)?;
        writer.write_all(b"check\r")?;
        writer.flush()?;

        let resize_start = output.len();
        let expected_size = format!("QWE_SIZE {rows} {cols}");
        let expected_winch = format!("QWE_WINCH {rows} {cols}");
        let observed_size = read_until(&reader, &mut output, &expected_size)?;
        let observed = String::from_utf8_lossy(&output[resize_start..]);
        let observed_winch = observed.contains(&expected_winch);
        checks.push((format!("{rows}x{cols}"), observed_size, observed_winch));
    }

    writer.write_all(b"exit\r")?;
    writer.flush()?;
    drop(writer);
    read_to_end(&reader, &mut output)?;
    let status = child.wait()?;

    if let Some(path) = log_path {
        fs::write(path, &output)?;
    }

    let size_checks = checks.iter().all(|(_, size, _)| *size);
    let winch_checks = checks.iter().all(|(_, _, winch)| *winch);
    let passed = ready && size_checks && winch_checks && status.success();
    let sizes_text = checks
        .iter()
        .map(|(size, _, _)| format!("\"{size}\""))
        .collect::<Vec<_>>()
        .join(",");

    println!(
        r#"{{"contract":"qwe-poc/0.1","feature":"pty-resize","implementation":"rust","exit_code":{},"bytes":{},"checks":{{"ready":{},"size":{},"winch":{}}},"sizes":[{}]}}"#,
        status.exit_code(),
        output.len(),
        ready,
        size_checks,
        winch_checks,
        sizes_text
    );
    Ok(passed)
}

fn main() -> ExitCode {
    let command = std::env::args().nth(1);

    match command.as_deref() {
        Some("probe") => {
            println!("{{\"contract\":\"qwe-poc/0.1\",\"implementation\":\"rust\"}}");
            ExitCode::SUCCESS
        }
        Some("pty-smoke") => {
            let log_path = std::env::args().nth(2);
            match run_pty_smoke(log_path.as_deref()) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => {
                    eprintln!("PTY smoke checks failed");
                    ExitCode::FAILURE
                }
                Err(error) => {
                    eprintln!("PTY smoke failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("editor-smoke") => {
            let log_path = std::env::args().nth(2);
            match run_editor_smoke(log_path.as_deref()) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => {
                    eprintln!("Neovim smoke checks failed");
                    ExitCode::FAILURE
                }
                Err(error) => {
                    eprintln!("Neovim smoke failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("resize-smoke") => {
            let log_path = std::env::args().nth(2);
            match run_resize_smoke(log_path.as_deref()) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => {
                    eprintln!("PTY resize checks failed");
                    ExitCode::FAILURE
                }
                Err(error) => {
                    eprintln!("PTY resize failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("help") | None => {
            println!("usage: qwe-poc <probe|pty-smoke|editor-smoke|resize-smoke> [log-path]");
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("unsupported command: {command}");
            ExitCode::from(2)
        }
    }
}
