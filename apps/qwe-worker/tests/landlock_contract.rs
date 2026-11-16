use qwe_runtime::landlock_abi_version;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TestRoot {
    path: PathBuf,
    workspace: PathBuf,
    forbidden: PathBuf,
}

impl TestRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("landlock-contract-{}-{nonce}", std::process::id()));
        let workspace = path.join("workspace");
        let forbidden = path.join("forbidden");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&forbidden).unwrap();
        Self {
            path,
            workspace,
            forbidden,
        }
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_init(root: &TestRoot, mode: &str, script: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_qwe-sandbox-init"))
        .current_dir(&root.workspace)
        .arg("--workspace")
        .arg(&root.workspace)
        .arg("--workspace-mode")
        .arg(mode)
        .arg("--private-tmp")
        .arg("--")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("qwe-landlock-test")
        .arg(&root.forbidden)
        .output()
        .unwrap()
}

#[test]
fn sandbox_init_failure_has_a_distinct_exit_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_qwe-sandbox-init"))
        .arg("--workspace")
        .arg("/definitely/missing/qwe-workspace")
        .arg("--workspace-mode")
        .arg("read-only")
        .arg("--private-tmp")
        .arg("--")
        .arg("/bin/true")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&output.stderr).contains("qwe-sandbox-init:"));
}

#[test]
fn sandbox_init_failure_never_executes_the_target() {
    let root = TestRoot::new();
    let marker = root.path.join("target-executed");
    for workspace in [
        PathBuf::from("/definitely/missing/qwe-workspace"),
        root.path.join("x".repeat(5000)),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_qwe-sandbox-init"))
            .arg("--workspace")
            .arg(&workspace)
            .arg("--workspace-mode")
            .arg("read-only")
            .arg("--private-tmp")
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg("printf executed > \"$1\"")
            .arg("qwe-sandbox-init-test")
            .arg(&marker)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(125));
        assert!(!marker.exists(), "workspace {}", workspace.display());
    }
}

#[test]
fn landlock_confines_writes_while_preserving_reads_and_private_tmp() {
    if landlock_abi_version().is_err() {
        return;
    }
    let root = TestRoot::new();
    let output = run_init(
        &root,
        "read-write",
        concat!(
            "set -eu; ",
            "printf allowed > allowed.txt; ",
            "if printf denied > \"$1/outside.txt\" 2>/dev/null; then exit 91; fi; ",
            "test -r /etc/os-release; ",
            "tmp_file=\"/tmp/qwe-landlock-contract-$$\"; ",
            "printf tmp > \"$tmp_file\"; rm -f \"$tmp_file\"; ",
            "printf 'LANDLOCK_ABI=%s' \"$QWE_LANDLOCK_ABI\""
        ),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("LANDLOCK_ABI="), "{stdout}");
    assert_eq!(
        fs::read(root.workspace.join("allowed.txt")).unwrap(),
        b"allowed"
    );
    assert!(!root.forbidden.join("outside.txt").exists());
}

#[test]
fn landlock_read_only_workspace_rejects_writes() {
    if landlock_abi_version().is_err() {
        return;
    }
    let root = TestRoot::new();
    let output = run_init(
        &root,
        "read-only",
        "if printf denied > allowed.txt 2>/dev/null; then exit 91; fi; printf READ_ONLY_OK",
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"READ_ONLY_OK");
    assert!(!Path::new(&root.workspace.join("allowed.txt")).exists());
}
