#![cfg(target_os = "linux")]

use qwe_core::{SandboxProfile, SandboxWorkspaceMode};
use qwe_protocol::{PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope, read_frame, write_frame};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct WorkerGuard {
    child: Child,
    root: PathBuf,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn bubblewrap_available() -> bool {
    Command::new("bwrap")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn connect_with_retry(path: &Path, timeout: Duration) -> UnixStream {
    let deadline = Instant::now() + timeout;
    loop {
        match UnixStream::connect(path) {
            Ok(stream) => return stream,
            Err(failure) if Instant::now() < deadline => {
                let _ = failure;
                thread::sleep(Duration::from_millis(20));
            }
            Err(failure) => panic!("worker socket did not become ready: {failure}"),
        }
    }
}

fn call(stream: &mut UnixStream, operation: &str, payload: Value) -> Value {
    let request = RequestEnvelope {
        version: PROTOCOL_VERSION,
        request_id: format!("{operation}-{}", std::process::id()),
        idempotency_key: None,
        operation: operation.into(),
        payload,
        auth_context: None,
    };
    write_frame(stream, &request).unwrap();
    let response = read_frame::<_, ResponseEnvelope>(stream).unwrap();
    response.result.unwrap()
}

fn wait_for_output(stream: &mut UnixStream, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = call(stream, "SESSION_ATTACH", json!({}));
        if status["screen"]
            .as_str()
            .is_some_and(|screen| screen.contains(marker))
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {marker}; status={status}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn worker_starts_and_controls_a_long_lived_sandboxed_pane() {
    if !bubblewrap_available() {
        eprintln!("SKIP: bwrap is unavailable");
        return;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("qwe-sp-worker-{}-{nonce}", std::process::id()));
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let socket = root.join("worker.sock");
    let mut profile = SandboxProfile::new(workspace.clone());
    profile.workspace_mode = SandboxWorkspaceMode::ReadWrite;
    let profile_json = serde_json::to_string(&profile).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_qwe-worker"))
        .arg("--worker-socket")
        .arg(&socket)
        .arg("--worker-session-id")
        .arg("sandboxed-pane-contract")
        .arg("--worker-pane-id")
        .arg("sandboxed-pane-contract")
        .arg("--worker-program")
        .arg("/bin/bash")
        .args(["--worker-arg", "--noprofile", "--worker-arg", "--norc"])
        .args(["--worker-arg", "-i"])
        .arg("--worker-sandbox-profile")
        .arg(profile_json)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let _guard = WorkerGuard {
        child,
        root: root.clone(),
    };
    let mut stream = connect_with_retry(&socket, Duration::from_secs(5));
    let hello = call(&mut stream, "WORKER_HELLO", json!({}));
    assert_eq!(hello["running"], true, "{hello}");
    assert_eq!(hello["sandbox"]["enforced"], true, "{hello}");
    assert_eq!(hello["sandbox"]["backend"], "bubblewrap", "{hello}");

    let command =
        "printf 'PANE_SANDBOX_OK:%s:%s\\n' \"$HOME\" \"$(pwd)\"; touch sandboxed-pane-marker\n";
    call(&mut stream, "SESSION_INPUT", json!({"data": command}));
    wait_for_output(&mut stream, "PANE_SANDBOX_OK:/nonexistent:");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !workspace.join("sandboxed-pane-marker").exists() {
        assert!(
            Instant::now() < deadline,
            "sandboxed Pane did not write its workspace marker"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let closed = call(&mut stream, "SESSION_CLOSE", json!({}));
    assert_eq!(closed["state"], "closed", "{closed}");
}
