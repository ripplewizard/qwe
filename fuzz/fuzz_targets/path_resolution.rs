#![no_main]

use libfuzzer_sys::fuzz_target;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

fn root() -> &'static Path {
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("qwe-path-fuzz-{}", std::process::id()));
        let inside = root.join("inside");
        fs::create_dir_all(&inside).expect("create fuzz root");
        fs::write(inside.join("file"), b"inside").expect("create in-root file");
        std::os::unix::fs::symlink("inside", root.join("relative-link"))
            .expect("create relative symlink");
        std::os::unix::fs::symlink("/etc/passwd", root.join("absolute-escape"))
            .expect("create escaping symlink");
        std::os::unix::fs::symlink("loop", root.join("loop")).expect("create symlink loop");
        let mut deep = root.to_path_buf();
        for index in 0..32 {
            deep.push(format!("d{index}"));
        }
        fs::create_dir_all(&deep).expect("create deep tree");
        fs::write(deep.join("file"), b"deep").expect("create deep file");
        root
    })
}

fuzz_target!(|data: &[u8]| {
    let data = &data[..data.len().min(8192)];
    let path = Path::new(OsStr::from_bytes(data));
    let _ = qwe_runtime::resolve_beneath(root(), path);
});
