#![cfg(target_os = "linux")]

use qwe_core::SandboxProfile;
use qwe_protocol::ResponseEnvelope;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn bubblewrap_available() -> bool {
    Command::new("bwrap")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn standalone_probe_returns_audit_without_an_existing_pane() {
    if !bubblewrap_available() {
        eprintln!("SKIP: bwrap is unavailable");
        return;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("sandbox-probe-cli-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&workspace).unwrap();
    let profile = SandboxProfile::new(workspace.clone());
    let profile_json = serde_json::to_string(&profile).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_qwe-worker"))
        .arg("--sandbox-probe-profile")
        .arg(profile_json)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = serde_json::from_slice::<ResponseEnvelope>(&output.stdout).unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["available"], true);
    assert!(result["path"].as_str().is_some());
    assert!(result["version"].as_str().is_some());
    assert_eq!(
        result["seccomp_profile_hash"].as_str().map(str::len),
        Some(64)
    );

    let _ = fs::remove_dir_all(workspace);
}
