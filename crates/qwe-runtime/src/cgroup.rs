use qwe_core::ResourcePolicy;
#[cfg(target_os = "linux")]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
pub fn systemd_scope_available() -> bool {
    if !Path::new("/usr/bin/systemd-run").is_file() || !Path::new("/usr/bin/systemctl").is_file() {
        return false;
    }
    std::process::Command::new("/usr/bin/systemctl")
        .args(["--user", "is-system-running"])
        .output()
        .ok()
        .is_some_and(|output| {
            output.status.success()
                || matches!(
                    String::from_utf8_lossy(&output.stdout).trim(),
                    "degraded" | "starting" | "maintenance"
                )
        })
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn systemd_scope_available() -> bool {
    false
}

#[cfg(target_os = "linux")]
pub fn current_cgroup_path() -> io::Result<PathBuf> {
    let contents = fs::read_to_string("/proc/self/cgroup")?;
    let relative = contents
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cgroup v2 entry is missing"))?;
    Ok(Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/')))
}

#[cfg(not(target_os = "linux"))]
pub fn current_cgroup_path() -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cgroup paths are only available on Linux",
    ))
}

#[cfg(target_os = "linux")]
pub fn apply_resource_policy(path: &Path, policy: &ResourcePolicy) -> io::Result<()> {
    write_limit(path, "memory.high", policy.memory_high_bytes)?;
    write_limit(path, "memory.max", policy.memory_max_bytes)?;
    let swap = if policy.memory_max_bytes.is_some() || policy.memory_high_bytes.is_some() {
        Some(policy.memory_swap_max_bytes.unwrap_or(0))
    } else {
        policy.memory_swap_max_bytes
    };
    write_limit(path, "memory.swap.max", swap)?;
    write_limit(path, "pids.max", policy.tasks_max)?;
    let cpu_path = path.join("cpu.max");
    if cpu_path.exists() {
        let cpu = policy
            .cpu_quota_percent
            .map(|percent| format!("{} 100000", u64::from(percent) * 1000))
            .unwrap_or_else(|| "max 100000".into());
        fs::write(&cpu_path, cpu).map_err(|failure| {
            io::Error::new(
                failure.kind(),
                format!("failed to write {}: {failure}", cpu_path.display()),
            )
        })?;
    } else if policy.cpu_quota_percent.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "cpu controller is not enabled for this scope",
        ));
    }
    let oom_group = path.join("memory.oom.group");
    fs::write(&oom_group, "1").map_err(|failure| {
        io::Error::new(
            failure.kind(),
            format!("failed to write {}: {failure}", oom_group.display()),
        )
    })
}

#[cfg(not(target_os = "linux"))]
pub fn apply_resource_policy(_path: &Path, _policy: &ResourcePolicy) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cgroup resource control is only available on Linux",
    ))
}

#[cfg(target_os = "linux")]
fn write_limit(path: &Path, file: &str, value: Option<u64>) -> io::Result<()> {
    let value = value.map_or_else(|| "max".to_string(), |value| value.to_string());
    let target = path.join(file);
    fs::write(&target, value).map_err(|failure| {
        io::Error::new(
            failure.kind(),
            format!("failed to write {}: {failure}", target.display()),
        )
    })
}

#[cfg(target_os = "linux")]
pub fn read_resource_usage(path: &Path) -> io::Result<(u64, u64, u64, u64)> {
    let memory_current = read_u64(path.join("memory.current"))?;
    let memory_peak = read_u64_optional(path.join("memory.peak"))?.unwrap_or(memory_current);
    let tasks_current = read_u64(path.join("pids.current"))?;
    let tasks_peak = read_u64_optional(path.join("pids.peak"))?.unwrap_or(tasks_current);
    Ok((memory_current, memory_peak, tasks_current, tasks_peak))
}

#[cfg(not(target_os = "linux"))]
pub fn read_resource_usage(_path: &Path) -> io::Result<(u64, u64, u64, u64)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cgroup resource metrics are only available on Linux",
    ))
}

#[cfg(target_os = "linux")]
pub fn read_cpu_usage_microseconds(path: &Path) -> io::Result<u64> {
    let stats = fs::read_to_string(path.join("cpu.stat"))?;
    let microseconds = stats
        .lines()
        .find_map(|line| line.strip_prefix("usage_usec "))
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cpu.stat has no usage_usec"))?;
    Ok(microseconds)
}

#[cfg(not(target_os = "linux"))]
pub fn read_cpu_usage_microseconds(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cgroup CPU metrics are only available on Linux",
    ))
}

#[cfg(target_os = "linux")]
pub fn read_enforcement_event(path: &Path) -> io::Result<Option<String>> {
    if let Ok(events) = fs::read_to_string(path.join("memory.events")) {
        let violated = events.lines().any(|line| {
            matches!(
                line.split_whitespace().collect::<Vec<_>>().as_slice(),
                ["max", value] | ["oom", value] | ["oom_kill", value]
                    if value.parse::<u64>().unwrap_or_default() > 0
            )
        });
        if violated {
            return Ok(Some("memory_max".into()));
        }
    }
    if let Ok(events) = fs::read_to_string(path.join("pids.events")) {
        let violated = events.lines().any(|line| {
            matches!(
                line.split_whitespace().collect::<Vec<_>>().as_slice(),
                ["max", value] if value.parse::<u64>().unwrap_or_default() > 0
            )
        });
        if violated {
            return Ok(Some("tasks_max".into()));
        }
    }
    Ok(None)
}

#[cfg(not(target_os = "linux"))]
pub fn read_enforcement_event(_path: &Path) -> io::Result<Option<String>> {
    Ok(None)
}

#[cfg(target_os = "linux")]
fn read_u64(path: PathBuf) -> io::Result<u64> {
    fs::read_to_string(path)?
        .trim()
        .parse()
        .map_err(|failure| io::Error::new(io::ErrorKind::InvalidData, failure))
}

#[cfg(target_os = "linux")]
fn read_u64_optional(path: PathBuf) -> io::Result<Option<u64>> {
    match fs::read_to_string(path) {
        Ok(value) => value
            .trim()
            .parse()
            .map(Some)
            .map_err(|failure| io::Error::new(io::ErrorKind::InvalidData, failure)),
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(failure) => Err(failure),
    }
}
