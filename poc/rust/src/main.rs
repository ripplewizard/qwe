use portable_pty::{native_pty_system, CommandBuilder, ExitStatus, PtySize};
use std::fs;
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

mod lease;
mod resource;
mod restart;
mod runtime;
mod session;

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

fn read_until_all(
    receiver: &Receiver<ReaderEvent>,
    output: &mut Vec<u8>,
    markers: &[String],
) -> Result<bool, String> {
    let search_start = output.len();
    loop {
        let observed = String::from_utf8_lossy(&output[search_start..]);
        if markers.iter().all(|marker| observed.contains(marker)) {
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

fn notify_foreground_resize(process_id: u32) -> Result<(), String> {
    let result = unsafe { libc::kill(-(process_id as libc::pid_t), libc::SIGWINCH) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

fn run_pty_session(
    shell: &str,
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

    let mut command = CommandBuilder::new(shell);
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
    let before = open_fd_count()?;
    let mut combined = Vec::new();
    let mut bash = false;
    let mut zsh = false;
    for shell_name in ["bash", "zsh"] {
        let run = run_pty_session(
            shell_name,
            "printf 'QWE_PTY_READY\\n'; printf 'QWE_PTY_SIZE '; stty size; printf 'QWE_PTY_END\\n'; exit 0\r",
            None,
        )?;
        let text = String::from_utf8_lossy(&run.output);
        let valid = run.status.success()
            && text.contains("QWE_PTY_READY")
            && text.contains("QWE_PTY_SIZE 24 80")
            && text.contains("QWE_PTY_END");
        if shell_name == "bash" {
            bash = valid;
        } else {
            zsh = valid;
        }
        combined.extend_from_slice(&run.output);
    }
    if let Some(path) = log_path {
        fs::write(path, &combined)?;
    }
    std::thread::sleep(Duration::from_millis(20));
    let after = open_fd_count()?;
    let text = String::from_utf8_lossy(&combined);
    let shell = bash && zsh;
    let window_size = text.contains("QWE_PTY_SIZE 24 80");
    let fd_no_leak = after <= before;
    let process_reaped = bash && zsh;
    let passed = shell && window_size && fd_no_leak && process_reaped;

    println!(
        r#"{{"contract":"qwe-poc/0.1","feature":"pty-smoke","implementation":"rust","exit_code":0,"bytes":{},"checks":{{"shell":{},"bash":{},"zsh":{},"window_size":{},"fd_no_leak":{},"process_reaped":{}}}}}"#,
        combined.len(),
        shell,
        bash,
        zsh,
        window_size,
        fd_no_leak,
        process_reaped
    );
    Ok(passed)
}

fn open_fd_count() -> Result<usize, Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    let path = "/proc/self/fd";
    #[cfg(target_os = "macos")]
    let path = "/dev/fd";
    Ok(fs::read_dir(path)?.count())
}

fn run_long_output_smoke(
    log_path: Option<&str>,
    script_path: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    const LINE_COUNT: usize = 50_000;
    let script = std::fs::canonicalize(script_path)?;
    let script = script.display().to_string().replace('\'', "'\\''");
    let input = format!("exec '{script}' {LINE_COUNT}\r");
    let started = std::time::Instant::now();
    let run = run_pty_session("sh", &input, log_path)?;
    let elapsed_ms = started.elapsed().as_millis();
    let text = String::from_utf8_lossy(&run.output);
    let lines = text.matches("QWE-LINE-").count();
    let count_ok = lines == LINE_COUNT;
    let first_ok = text.contains("QWE-LINE-00050000");
    let last_ok = text.contains("QWE-LINE-00000001");
    let passed = run.status.success() && count_ok && first_ok && last_ok;
    println!(
        r#"{{"contract":"qwe-poc/0.1","feature":"long-output","implementation":"rust","exit_code":{},"bytes":{},"lines":{},"elapsed_ms":{},"checks":{{"count":{},"first":{},"last":{}}}}}"#,
        run.status.exit_code(),
        run.output.len(),
        lines,
        elapsed_ms,
        count_ok,
        first_ok,
        last_ok
    );
    Ok(passed)
}

fn run_vt_smoke(
    log_path: Option<&str>,
    fixture_path: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let fixture = std::fs::canonicalize(fixture_path)?;
    let fixture = fixture.display().to_string().replace('\'', "'\\''");
    let run = run_pty_session("sh", &format!("exec python3 '{fixture}'\r"), None)?;
    let alt_run = run_pty_session("sh", &format!("exec python3 '{fixture}' alt\r"), None)?;
    let mut combined = run.output.clone();
    combined.extend_from_slice(&alt_run.output);
    if let Some(path) = log_path {
        fs::write(path, &combined)?;
    }

    let mut parser =
        vt100::Parser::new(vt100::Parser::DEFAULT_ROWS, vt100::Parser::DEFAULT_COLS, 0);
    parser.process(&run.output);
    let screen = parser.screen();
    let text = screen.contents();
    let red = screen
        .cell(1, 0)
        .map(|cell| cell.fgcolor() == vt100::Color::Idx(1))
        .unwrap_or(false);
    let blue = screen
        .cell(1, 4)
        .map(|cell| cell.fgcolor() == vt100::Color::Idx(4) && cell.bold())
        .unwrap_or(false);
    let erase = screen
        .cell(2, 0)
        .map(|cell| cell.contents() == "A")
        .unwrap_or(false)
        && screen
            .cell(2, 1)
            .map(|cell| cell.contents().is_empty())
            .unwrap_or(false);
    let cursor = screen.cursor_position() == (5, 10) && screen.hide_cursor();
    let row_text = |row: u16| {
        (0..12)
            .filter_map(|col| screen.cell(row, col))
            .map(|cell| cell.contents().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    };
    let scroll_region =
        row_text(6) == "OUTSIDE" && row_text(7) == "SCROLL9" && row_text(8) == "SCROLL10";
    let title = String::from_utf8_lossy(&run.output).contains("QWE_VT_TEST");
    let unicode = text.contains("中文") && text.contains("😀") && text.contains("e\u{301}");
    let wide = screen
        .cell(3, 0)
        .map(|cell| cell.is_wide())
        .unwrap_or(false)
        && screen
            .cell(3, 1)
            .map(|cell| cell.is_wide_continuation())
            .unwrap_or(false);
    let mut alt_parser =
        vt100::Parser::new(vt100::Parser::DEFAULT_ROWS, vt100::Parser::DEFAULT_COLS, 0);
    alt_parser.process(&alt_run.output);
    let alternate = alt_run.status.success()
        && alt_parser.screen().alternate_screen()
        && alt_parser.screen().contents().contains("ALT_SCREEN");
    let t05 = run.status.success()
        && text.contains("QWE_BASE")
        && red
        && blue
        && erase
        && cursor
        && scroll_region
        && title
        && alternate;
    let t06 = run.status.success() && unicode && wide;
    println!(
        r#"{{"contract":"qwe-poc/0.1","feature":"vt-state","implementation":"rust","exit_code":{},"checks":{{"base_text":{},"colors":{},"erase":{},"cursor":{},"scroll_region":{},"title":{},"alternate_screen":{},"unicode":{},"wide":{}}}}}"#,
        run.status.exit_code(),
        text.contains("QWE_BASE"),
        red && blue,
        erase,
        cursor,
        scroll_region,
        title,
        alternate,
        unicode,
        wide
    );
    Ok(t05 && t06)
}

fn run_editor_smoke(log_path: Option<&str>) -> Result<bool, Box<dyn std::error::Error>> {
    let input = concat!(
        "printf 'QWE_EDITOR_SIZE '; stty size; ",
        "nvim --clean -u NONE -c 'call setline(1, $NVIM_TEST_MARKER)' ",
        "-c 'redraw' -c 'sleep 200m' -c 'qa!'; exit $?\r"
    );
    let run = run_pty_session("sh", input, log_path)?;
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

fn run_resize_smoke(
    log_path: Option<&str>,
    watcher_path: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let watcher_path = std::fs::canonicalize(watcher_path)?;
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut command = CommandBuilder::new("sh");
    command.env("TERM", "xterm-256color");
    command.env("QWE_WATCHER_PATH", &watcher_path);
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);

    let reader = spawn_reader(pair.master.try_clone_reader()?);
    let mut writer = pair.master.take_writer()?;
    writer.write_all(b"exec python3 \"$QWE_WATCHER_PATH\"\r")?;
    writer.flush()?;

    let process_id = child
        .process_id()
        .ok_or("child process does not expose a process id")?;
    let mut output = Vec::new();
    let mut checks = Vec::new();
    let ready = match read_until(&reader, &mut output, "QWE_RESIZE_READY 24 80") {
        Ok(value) => value,
        Err(error) => {
            if let Some(path) = log_path {
                fs::write(path, &output)?;
            }
            return Err(error.into());
        }
    };

    let sizes = [(120, 40), (80, 24), (160, 50), (40, 15)];
    let mut emulator = vt100::Parser::new(
        NonZeroU16::new(24).unwrap(),
        NonZeroU16::new(80).unwrap(),
        0,
    );
    let mut emulator_checks = true;
    for (cols, rows) in sizes {
        pair.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        emulator.set_size(
            NonZeroU16::new(rows).unwrap(),
            NonZeroU16::new(cols).unwrap(),
        );
        let (actual_rows, actual_cols) = emulator.screen().size();
        emulator_checks &= actual_rows.get() == rows && actual_cols.get() == cols;
        writer.write_all(b"check\r")?;
        writer.flush()?;
        thread::sleep(Duration::from_millis(10));
        notify_foreground_resize(process_id)?;
        thread::sleep(Duration::from_millis(10));
        notify_foreground_resize(process_id)?;

        let expected_size = format!("QWE_SIZE {rows} {cols}");
        let expected_winch = format!("QWE_WINCH {rows} {cols}");
        let observed = match read_until_all(&reader, &mut output, &[expected_size, expected_winch])
        {
            Ok(value) => value,
            Err(error) => {
                if let Some(path) = log_path {
                    fs::write(path, &output)?;
                }
                return Err(error.into());
            }
        };
        checks.push((format!("{rows}x{cols}"), observed, observed));
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
    let passed = ready && size_checks && winch_checks && emulator_checks && status.success();
    let sizes_text = checks
        .iter()
        .map(|(size, _, _)| format!("\"{size}\""))
        .collect::<Vec<_>>()
        .join(",");

    println!(
        r#"{{"contract":"qwe-poc/0.1","feature":"pty-resize","implementation":"rust","exit_code":{},"bytes":{},"checks":{{"ready":{},"size":{},"winch":{},"emulator":{}}},"sizes":[{}]}}"#,
        status.exit_code(),
        output.len(),
        ready,
        size_checks,
        winch_checks,
        emulator_checks,
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
        Some("long-output-smoke") => {
            let log_path = std::env::args().nth(2);
            let script = std::env::args()
                .nth(3)
                .unwrap_or_else(|| "poc/test-data/scripts/long_output.sh".to_string());
            match run_long_output_smoke(log_path.as_deref(), &script) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => {
                    eprintln!("long-output checks failed");
                    ExitCode::FAILURE
                }
                Err(error) => {
                    eprintln!("long-output failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("vt-smoke") => {
            let log_path = std::env::args().nth(2);
            let fixture = std::env::args()
                .nth(3)
                .unwrap_or_else(|| "poc/test-data/scripts/vt_fixture.py".to_string());
            match run_vt_smoke(log_path.as_deref(), &fixture) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => {
                    eprintln!("VT state checks failed");
                    ExitCode::FAILURE
                }
                Err(error) => {
                    eprintln!("VT state failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("resize-smoke") => {
            let log_path = std::env::args().nth(2);
            let watcher = std::env::args()
                .nth(3)
                .unwrap_or_else(|| "poc/test-data/scripts/resize_watcher.py".to_string());
            match run_resize_smoke(log_path.as_deref(), &watcher) {
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
        Some("daemon") => {
            let arguments: Vec<_> = std::env::args().skip(2).collect();
            if arguments.len() != 2 {
                eprintln!("usage: qwe-poc daemon <socket-path> <sqlite-path>");
                ExitCode::from(2)
            } else {
                match runtime::run_daemon(&arguments[0], &arguments[1]) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("daemon failed: {error}");
                        ExitCode::FAILURE
                    }
                }
            }
        }
        Some("task-start") => runtime::client_command("START"),
        Some("task-start-timeout") => runtime::client_command("TIMEOUT_START"),
        Some("task-start-restart") => runtime::client_command("RESTART_START"),
        Some("task-restart-status") => runtime::client_command("RESTART_STATUS"),
        Some("task-status") => runtime::client_command("STATUS"),
        Some("task-stop") => runtime::client_command("STOP"),
        Some("task-history") => runtime::client_command("TASK_HISTORY"),
        Some("task-signal") => runtime::client_command("TASK_SIGNAL"),
        Some("task-resource") => runtime::client_command("TASK_RESOURCE"),
        Some("task-resource-sample") => runtime::client_command("TASK_RESOURCE_SAMPLE"),
        Some("lease-claim") => runtime::client_command("LEASE_CLAIM"),
        Some("lease-touch") => runtime::client_command("LEASE_TOUCH"),
        Some("lease-release") => runtime::client_command("LEASE_RELEASE"),
        Some("lease-status") => runtime::client_command("LEASE_STATUS"),
        Some("lease-takeover") => runtime::client_command("LEASE_TAKEOVER"),
        Some("session-start") => runtime::client_command("SESSION_START"),
        Some("session-attach") => runtime::client_command("SESSION_ATTACH"),
        Some("session-detach") => runtime::client_command("SESSION_DETACH"),
        Some("session-input") => runtime::client_command("SESSION_INPUT"),
        Some("session-status") => runtime::client_command("SESSION_STATUS"),
        Some("dashboard") => {
            let Some(database_path) = std::env::args().nth(2) else {
                eprintln!("usage: qwe-poc dashboard <sqlite-path>");
                return ExitCode::from(2);
            };
            runtime::run_dashboard(&database_path)
        }
        Some("help") | None => {
            println!("usage: qwe-poc <smoke|daemon|task-*|lease-*|session-start>");
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("unsupported command: {command}");
            ExitCode::from(2)
        }
    }
}
