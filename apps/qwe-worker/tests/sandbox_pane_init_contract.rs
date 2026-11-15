#![cfg(target_os = "linux")]

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use qwe_core::SANDBOXED_PANE_SHUTDOWN_GRACE_MS;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn bubblewrap_available() -> bool {
    Command::new("bwrap")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn process_with_arg_exists(marker: &str) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        if !name
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            return false;
        }
        fs::read(entry.path().join("cmdline")).is_ok_and(|cmdline| {
            cmdline
                .split(|byte| *byte == 0)
                .any(|part| part == marker.as_bytes())
        })
    })
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
        let chunk = receiver.recv_timeout(remaining).unwrap_or_else(|_| {
            panic!(
                "timed out waiting for {marker:?}; output={}",
                String::from_utf8_lossy(output)
            )
        });
        output.extend_from_slice(&chunk);
    }
}

fn signal_shim_and_expect(
    writer: &mut impl Write,
    receiver: &std::sync::mpsc::Receiver<Vec<u8>>,
    output: &mut Vec<u8>,
    signal: &str,
    marker: &[u8],
) {
    writer
        .write_all(format!("kill -{signal} $PPID\n").as_bytes())
        .unwrap();
    writer.flush().unwrap();
    read_until(receiver, output, marker, Duration::from_secs(2));
}

#[test]
fn pane_init_owns_terminal_forwards_signals_and_reaps_namespace() {
    if !bubblewrap_available() {
        eprintln!("SKIP: bwrap is unavailable");
        return;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("sandbox-pane-init-{}-{nonce}", std::process::id()));
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let init = fs::canonicalize(env!("CARGO_BIN_EXE_qwe-sandbox-init")).unwrap();
    let workspace_arg = workspace.to_str().unwrap();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut builder = CommandBuilder::new("bwrap");
    builder.args([
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
        "--clearenv",
        "--setenv",
        "PATH",
        "/usr/bin:/bin",
        "--ro-bind",
    ]);
    builder.arg(init.as_os_str());
    builder.arg("/qwe-sandbox-init");
    builder.args(["--bind", workspace_arg, workspace_arg]);
    builder.args(["--chdir", workspace_arg, "--"]);
    builder.args([
        "/qwe-sandbox-init",
        "--pane-init",
        "--workspace",
        workspace_arg,
        "--workspace-mode",
        "read-write",
        "--private-tmp",
        "--",
        "/bin/bash",
        "--noprofile",
        "--norc",
        "-i",
    ]);
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
    writer
        .write_all(
            b"trap 'printf \"\\nTERM_FORWARDED\\n\"' TERM; trap 'printf \"\\nINT_FORWARDED\\n\"' INT; trap 'printf \"\\nHUP_FORWARDED\\n\"' HUP; trap 'printf \"\\nWINCH_FORWARDED\\n\"' WINCH; printf 'PANE_READY pid=%s pgrp=%s tpgid=%s\\n' \"$$\" \"$(ps -o pgid= -p $$ | tr -d ' ')\" \"$(ps -o tpgid= -p $$ | tr -d ' ')\"\n",
        )
        .unwrap();
    writer.flush().unwrap();
    read_until(
        &receiver,
        &mut output,
        b"PANE_READY pid=2 pgrp=2 tpgid=2",
        Duration::from_secs(3),
    );

    signal_shim_and_expect(
        &mut writer,
        &receiver,
        &mut output,
        "TERM",
        b"TERM_FORWARDED",
    );
    signal_shim_and_expect(&mut writer, &receiver, &mut output, "INT", b"INT_FORWARDED");
    signal_shim_and_expect(&mut writer, &receiver, &mut output, "HUP", b"HUP_FORWARDED");

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
        b"WINCH_FORWARDED",
        Duration::from_secs(2),
    );

    writer
        .write_all(
            b"/bin/bash -c '/bin/sleep 0.05 &'\nfor _ in {1..100}; do children=\"$(cat /proc/1/task/1/children)\"; [ \"$children\" = \"$$\" ] && { printf 'ORPHANS_REAPED\\n'; break; }; /bin/sleep 0.02; done\n",
        )
        .unwrap();
    writer.flush().unwrap();
    read_until(
        &receiver,
        &mut output,
        b"ORPHANS_REAPED",
        Duration::from_secs(3),
    );

    writer
        .write_all(
            b"/bin/bash -c 'trap \"\" TERM HUP; while :; do /bin/sleep 1; done' qwe-pane-bg-marker & disown\n",
        )
        .unwrap();
    writer.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !process_with_arg_exists("qwe-pane-bg-marker") {
        assert!(Instant::now() < deadline, "background marker did not start");
        thread::sleep(Duration::from_millis(20));
    }

    let teardown_started = Instant::now();
    writer.write_all(b"exit 0\n").unwrap();
    writer.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("pane shim did not finish bounded sandbox teardown");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(
        teardown_started.elapsed()
            >= Duration::from_millis(SANDBOXED_PANE_SHUTDOWN_GRACE_MS.saturating_sub(500)),
        "detached TERM/HUP-ignoring process did not exercise the bounded teardown grace period"
    );
    assert_eq!(status.exit_code(), 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_with_arg_exists("qwe-pane-bg-marker") {
        assert!(
            Instant::now() < deadline,
            "background namespace process survived shell exit"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_dir_all(root);
}
