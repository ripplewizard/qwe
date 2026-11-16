#![cfg(all(target_os = "linux", feature = "sandbox-fault-injection"))]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const STAGES: &[&str] = &[
    "after_no_new_privs",
    "after_ruleset",
    "after_tmp_rule",
    "after_dev_rule",
    "after_workspace_rule",
    "after_dumpability",
    "before_exec",
];

#[test]
fn every_injected_setup_failure_exits_125_without_exec() {
    if qwe_runtime::landlock_abi_version().is_err() {
        eprintln!("SKIP: Landlock is unavailable");
        return;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("sandbox-fault-{}-{nonce}", std::process::id()));
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let marker = root.join("target-executed");

    for stage in STAGES {
        let output = Command::new(env!("CARGO_BIN_EXE_qwe-sandbox-init"))
            .env(qwe_runtime::SANDBOX_FAULT_ENV, stage)
            .arg("--workspace")
            .arg(&workspace)
            .arg("--workspace-mode")
            .arg("read-write")
            .arg("--private-tmp")
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg("printf executed > \"$1\"")
            .arg("qwe-sandbox-fault")
            .arg(&marker)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(125),
            "stage {stage}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!marker.exists(), "stage {stage} executed the target");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(stage),
            "stage {stage}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(root).unwrap();
}
