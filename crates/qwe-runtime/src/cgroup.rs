use qwe_core::ResourcePolicy;
#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResourceSupport {
    pub cgroup_v2: bool,
    pub systemd_user: bool,
    pub memory: bool,
    pub cpu: bool,
    pub pids: bool,
}

impl ResourceSupport {
    #[must_use]
    pub fn hard_scope_available(self) -> bool {
        self.cgroup_v2 && self.systemd_user && self.memory && self.cpu && self.pids
    }

    #[must_use]
    pub fn hard_memory(self) -> bool {
        self.cgroup_v2 && self.systemd_user && self.memory
    }

    #[must_use]
    pub fn hard_cpu(self) -> bool {
        self.cgroup_v2 && self.systemd_user && self.cpu
    }

    #[must_use]
    pub fn hard_pids(self) -> bool {
        self.cgroup_v2 && self.systemd_user && self.pids
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CgroupStats {
    pub memory_events: Vec<(String, u64)>,
    pub pids_events: Vec<(String, u64)>,
    pub cpu_stat: Vec<(String, u64)>,
    pub memory_pressure: Option<Pressure>,
    pub cpu_pressure: Option<Pressure>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pressure {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    pub total_us: u64,
}

#[cfg(target_os = "linux")]
pub fn detect_resource_support() -> ResourceSupport {
    let current_cgroup = current_cgroup_path().ok();
    let cgroup_v2 =
        Path::new("/sys/fs/cgroup/cgroup.controllers").is_file() && current_cgroup.is_some();
    let mut support = ResourceSupport {
        cgroup_v2,
        systemd_user: systemd_user_available(),
        ..ResourceSupport::default()
    };
    if cgroup_v2 {
        let controllers = fs::read_to_string("/sys/fs/cgroup/cgroup.controllers")
            .map(|contents| parse_controllers(&contents))
            .unwrap_or_default();
        support.memory = controllers.contains("memory");
        support.cpu = controllers.contains("cpu");
        support.pids = controllers.contains("pids");
    }
    support
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn detect_resource_support() -> ResourceSupport {
    ResourceSupport::default()
}

#[cfg(target_os = "linux")]
fn systemd_user_available() -> bool {
    if !Path::new("/usr/bin/systemd-run").is_file() || !Path::new("/usr/bin/systemctl").is_file() {
        return false;
    }
    let runtime_dir = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }));
    if runtime_dir.join("systemd/private").exists() {
        return true;
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

#[cfg(any(target_os = "linux", test))]
fn parse_controllers(contents: &str) -> std::collections::BTreeSet<String> {
    contents.split_whitespace().map(str::to_string).collect()
}

#[cfg(target_os = "linux")]
pub fn systemd_scope_available() -> bool {
    detect_resource_support().hard_scope_available()
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
pub fn read_effective_resource_policy(path: &Path) -> io::Result<ResourcePolicy> {
    if !path.join("memory.max").is_file() || !path.join("pids.max").is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("cgroup control files are gone from {}", path.display()),
        ));
    }
    let cpu_quota_percent = if let Ok(value) = fs::read_to_string(path.join("cpu.max")) {
        let fields = value.split_whitespace().collect::<Vec<_>>();
        if fields.first() == Some(&"max") {
            None
        } else {
            let quota = fields
                .first()
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "cpu.max has an invalid quota")
                })?;
            let period = fields
                .get(1)
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "cpu.max has an invalid period")
                })?;
            Some(
                u32::try_from(
                    quota
                        .saturating_mul(100)
                        .saturating_add(period / 2)
                        .checked_div(period)
                        .unwrap_or_default(),
                )
                .map_err(|failure| io::Error::new(io::ErrorKind::InvalidData, failure))?,
            )
        }
    } else {
        None
    };
    Ok(ResourcePolicy {
        memory_high_bytes: read_limit(path, "memory.high")?,
        memory_max_bytes: read_limit(path, "memory.max")?,
        memory_swap_max_bytes: read_limit(path, "memory.swap.max")?,
        tasks_max: read_limit(path, "pids.max")?,
        cpu_quota_percent,
        cpu_seconds: None,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn read_effective_resource_policy(_path: &Path) -> io::Result<ResourcePolicy> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "effective cgroup policy is only available on Linux",
    ))
}

#[cfg(target_os = "linux")]
pub fn read_cgroup_stats(path: &Path) -> io::Result<CgroupStats> {
    if ![
        "memory.events",
        "pids.events",
        "cpu.stat",
        "memory.pressure",
        "cpu.pressure",
    ]
    .into_iter()
    .any(|file| path.join(file).is_file())
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("cgroup statistics are gone from {}", path.display()),
        ));
    }
    Ok(CgroupStats {
        memory_events: read_counter_file(&path.join("memory.events"))?,
        pids_events: read_counter_file(&path.join("pids.events"))?,
        cpu_stat: read_counter_file(&path.join("cpu.stat"))?,
        memory_pressure: read_pressure(&path.join("memory.pressure"))?,
        cpu_pressure: read_pressure(&path.join("cpu.pressure"))?,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn read_cgroup_stats(_path: &Path) -> io::Result<CgroupStats> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cgroup statistics are only available on Linux",
    ))
}

#[must_use]
pub fn pane_scope_unit(pane_id: &str) -> String {
    format!("qwe-pane-{pane_id}.scope")
}

#[cfg(target_os = "linux")]
pub fn stop_systemd_scope(scope: &str) -> io::Result<()> {
    let status = std::process::Command::new("/usr/bin/systemctl")
        .args(["--user", "stop", scope])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    let _ = std::process::Command::new("/usr/bin/systemctl")
        .args(["--user", "reset-failed", scope])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "systemctl --user stop {scope} failed with {status}"
        )))
    }
}

#[cfg(not(target_os = "linux"))]
pub fn stop_systemd_scope(_scope: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "systemd user scopes are only available on Linux",
    ))
}

pub fn stop_pane_scope(pane_id: &str) -> io::Result<()> {
    stop_systemd_scope(&pane_scope_unit(pane_id))
}

#[cfg(target_os = "linux")]
pub fn kill_cgroup(cgroup: &Path) -> io::Result<()> {
    let kill_path = cgroup.join("cgroup.kill");
    if kill_path.exists() {
        return fs::write(kill_path, b"1");
    }
    let processes = fs::read_to_string(cgroup.join("cgroup.procs"))?;
    for pid in processes
        .split_whitespace()
        .filter_map(|value| value.parse::<libc::pid_t>().ok())
    {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn kill_cgroup(_cgroup: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cgroup killing is only available on Linux",
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

#[cfg(target_os = "linux")]
fn read_limit(path: &Path, file: &str) -> io::Result<Option<u64>> {
    let target = path.join(file);
    match fs::read_to_string(&target) {
        Ok(value) if value.trim() == "max" => Ok(None),
        Ok(value) => value.trim().parse().map(Some).map_err(|failure| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {failure}", target.display()),
            )
        }),
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(failure) => Err(failure),
    }
}

#[cfg(target_os = "linux")]
fn read_counter_file(path: &Path) -> io::Result<Vec<(String, u64)>> {
    let mut counters = BTreeMap::new();
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(failure) => return Err(failure),
    };
    for line in contents.lines() {
        let Some((name, value)) = line.split_once(' ') else {
            continue;
        };
        if let Ok(value) = value.trim().parse::<u64>() {
            counters.insert(name.to_string(), value);
        }
    }
    Ok(counters.into_iter().collect())
}

#[cfg(target_os = "linux")]
fn read_pressure(path: &Path) -> io::Result<Option<Pressure>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(failure) => return Err(failure),
    };
    Ok(parse_pressure(&contents))
}

#[cfg(any(target_os = "linux", test))]
fn parse_pressure(contents: &str) -> Option<Pressure> {
    let some = contents.lines().find(|line| line.starts_with("some "))?;
    let mut pressure = Pressure::default();
    for field in some.split_whitespace().skip(1) {
        let Some((name, value)) = field.split_once('=') else {
            continue;
        };
        match name {
            "avg10" => pressure.avg10 = value.parse().unwrap_or_default(),
            "avg60" => pressure.avg60 = value.parse().unwrap_or_default(),
            "avg300" => pressure.avg300 = value.parse().unwrap_or_default(),
            "total" => pressure.total_us = value.parse().unwrap_or_default(),
            _ => {}
        }
    }
    Some(pressure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_controller_and_pressure_files() {
        let controllers = parse_controllers("cpu memory pids io\nhugetlb\n");
        assert!(controllers.contains("cpu"));
        assert!(controllers.contains("memory"));
        assert!(controllers.contains("pids"));
        assert!(!controllers.contains("rdma"));

        let pressure = parse_pressure(
            "some avg10=1.25 avg60=0.50 avg300=0.10 total=1234\n\
             full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
        )
        .unwrap();
        assert_eq!(pressure.avg10, 1.25);
        assert_eq!(pressure.avg60, 0.5);
        assert_eq!(pressure.avg300, 0.1);
        assert_eq!(pressure.total_us, 1234);
    }

    #[test]
    fn resource_support_requires_every_controller_for_a_hard_scope() {
        let complete = ResourceSupport {
            cgroup_v2: true,
            systemd_user: true,
            memory: true,
            cpu: true,
            pids: true,
        };
        assert!(complete.hard_scope_available());
        assert!(
            !ResourceSupport {
                cpu: false,
                ..complete
            }
            .hard_scope_available()
        );
        assert!(
            !ResourceSupport {
                systemd_user: false,
                ..complete
            }
            .hard_memory()
        );
    }

    #[test]
    fn pane_scope_names_are_stable() {
        assert_eq!(
            pane_scope_unit("01900000-0000-7000-8000-000000000001"),
            "qwe-pane-01900000-0000-7000-8000-000000000001.scope"
        );
    }
}
