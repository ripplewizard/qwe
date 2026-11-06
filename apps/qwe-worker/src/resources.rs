use qwe_core::ResourcePolicy;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Default)]
pub struct ResourceMetrics {
    pub memory_current_bytes: u64,
    pub memory_peak_bytes: u64,
    pub tasks_current: u64,
    pub tasks_peak: u64,
    pub cpu_microseconds: u64,
    pub sampled_at_ms: u64,
    pub last_violation: Option<String>,
}

impl ResourceMetrics {
    #[must_use]
    pub fn json(&self, tier: &str) -> Value {
        json!({
            "tier": tier,
            "memory_current_bytes": self.memory_current_bytes,
            "memory_peak_bytes": self.memory_peak_bytes,
            "tasks_current": self.tasks_current,
            "tasks_peak": self.tasks_peak,
            "cpu_seconds": self.cpu_microseconds as f64 / 1_000_000.0,
            "sampled_at_ms": self.sampled_at_ms,
            "last_violation": self.last_violation
        })
    }
}

#[derive(Clone)]
pub struct ResourceController {
    policy: Arc<Mutex<ResourcePolicy>>,
    metrics: Arc<Mutex<ResourceMetrics>>,
    cgroup_dir: Option<PathBuf>,
}

impl ResourceController {
    pub fn new(policy: ResourcePolicy, cgroup_dir: Option<PathBuf>) -> Self {
        Self {
            policy: Arc::new(Mutex::new(policy)),
            metrics: Arc::new(Mutex::new(ResourceMetrics::default())),
            cgroup_dir,
        }
    }

    pub fn update(&self, policy: ResourcePolicy) -> std::io::Result<()> {
        if let Some(cgroup) = &self.cgroup_dir {
            let result = qwe_runtime::apply_resource_policy(cgroup, &policy).map_err(|failure| {
                std::io::Error::new(failure.kind(), format!("{}: {failure}", cgroup.display()))
            });
            if result
                .as_ref()
                .is_err_and(|failure| failure.kind() == std::io::ErrorKind::NotFound)
                && policy.cpu_quota_percent.is_some()
            {
                enable_cpu_quota(cgroup, policy.cpu_quota_percent.unwrap_or_default())?;
                qwe_runtime::apply_resource_policy(cgroup, &policy)?;
            } else {
                result?;
            }
        }
        *self
            .policy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = policy;
        Ok(())
    }

    #[must_use]
    pub fn metrics_json(&self) -> Value {
        let metrics = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        metrics.json(if self.cgroup_dir.is_some() {
            "hard"
        } else {
            "monitored"
        })
    }

    #[must_use]
    pub fn policy_json(&self) -> Value {
        serde_json::to_value(
            self.policy
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        )
        .unwrap_or(Value::Null)
    }

    pub fn start_monitor(&self, pid: u32, pgid: u32) {
        let controller = self.clone();
        thread::spawn(move || controller.monitor(pid, pgid));
    }

    pub fn cleanup_cgroup(&self) {
        if let Some(cgroup) = &self.cgroup_dir {
            let _ = std::fs::remove_dir(cgroup);
        }
    }

    pub fn capture_final_sample(&self) {
        #[cfg(target_os = "linux")]
        let _ = self.sample_linux();
    }

    fn monitor(&self, pid: u32, pgid: u32) {
        loop {
            let alive = process_alive(pid);
            #[cfg(target_os = "linux")]
            {
                let _ = self.sample_linux();
            }
            #[cfg(target_os = "macos")]
            let sampled = self.sample_macos(pid).is_ok();
            #[cfg(target_os = "macos")]
            if !sampled {
                if !alive {
                    break;
                }
                thread::sleep(SAMPLE_INTERVAL);
                continue;
            }
            if let Some(reason) = self.violation() {
                unsafe {
                    libc::kill(-(pgid as libc::pid_t), libc::SIGKILL);
                }
                self.metrics
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .last_violation = Some(reason);
                break;
            }
            if !alive {
                break;
            }
            thread::sleep(SAMPLE_INTERVAL);
        }
    }

    #[cfg(target_os = "linux")]
    fn sample_linux(&self) -> std::io::Result<()> {
        let Some(cgroup) = &self.cgroup_dir else {
            return Ok(());
        };
        let (memory_current, memory_peak, tasks_current, tasks_peak) =
            qwe_runtime::read_resource_usage(cgroup)?;
        let cpu_microseconds = qwe_runtime::read_cpu_usage_microseconds(cgroup)?;
        let mut metrics = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        metrics.memory_current_bytes = memory_current;
        metrics.memory_peak_bytes = metrics.memory_peak_bytes.max(memory_peak);
        metrics.tasks_current = tasks_current;
        metrics.tasks_peak = metrics.tasks_peak.max(tasks_peak);
        metrics.cpu_microseconds = cpu_microseconds;
        metrics.sampled_at_ms = now_ms();
        if let Ok(Some(event)) = qwe_runtime::read_enforcement_event(cgroup) {
            metrics.last_violation.get_or_insert(event);
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn sample_macos(&self, pid: u32) -> std::io::Result<()> {
        let tree = process_tree(pid)?;
        let mut memory = 0_u64;
        let mut cpu_nanos = 0_u64;
        for pid in &tree {
            let mut usage = unsafe { std::mem::zeroed::<libc::rusage_info_v4>() };
            if unsafe {
                libc::proc_pid_rusage(
                    *pid as libc::c_int,
                    libc::RUSAGE_INFO_V4,
                    (&mut usage as *mut libc::rusage_info_v4).cast(),
                )
            } == 0
            {
                memory = memory.saturating_add(usage.ri_phys_footprint);
                cpu_nanos = cpu_nanos
                    .saturating_add(usage.ri_user_time)
                    .saturating_add(usage.ri_system_time);
            }
        }
        let mut metrics = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        metrics.memory_current_bytes = memory;
        metrics.memory_peak_bytes = metrics.memory_peak_bytes.max(memory);
        metrics.tasks_current = tree.len() as u64;
        metrics.tasks_peak = metrics.tasks_peak.max(tree.len() as u64);
        metrics.cpu_microseconds = mach_ticks_to_nanos(cpu_nanos) / 1_000;
        metrics.sampled_at_ms = now_ms();
        Ok(())
    }

    fn violation(&self) -> Option<String> {
        let policy = self
            .policy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let metrics = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(reason) = metrics.last_violation {
            return Some(reason);
        }
        if policy
            .memory_max_bytes
            .is_some_and(|maximum| metrics.memory_current_bytes > maximum)
        {
            return Some("memory_max".into());
        }
        if policy
            .tasks_max
            .is_some_and(|maximum| metrics.tasks_current > maximum)
        {
            return Some("tasks_max".into());
        }
        if policy
            .cpu_seconds
            .is_some_and(|maximum| metrics.cpu_microseconds > maximum.saturating_mul(1_000_000))
        {
            return Some("cpu_seconds".into());
        }
        None
    }
}

#[cfg(target_os = "macos")]
fn process_tree(root: u32) -> std::io::Result<Vec<u32>> {
    fn collect(pid: u32, output: &mut Vec<u32>) -> std::io::Result<()> {
        output.push(pid);
        let mut children = vec![0_i32; 256];
        let count = unsafe {
            libc::proc_listchildpids(
                pid as libc::pid_t,
                children.as_mut_ptr().cast(),
                (children.len() * size_of::<i32>()) as libc::c_int,
            )
        };
        if count < 0 {
            return Err(std::io::Error::last_os_error());
        }
        for child in children.into_iter().take(count as usize) {
            if child > 0 {
                collect(child as u32, output)?;
            }
        }
        Ok(())
    }

    let mut output = Vec::new();
    collect(root, &mut output)?;
    Ok(output)
}

fn process_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn enable_cpu_quota(cgroup: &std::path::Path, percent: u32) -> std::io::Result<()> {
    let scope = cgroup
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid scope path")
        })?;
    let status = std::process::Command::new("/usr/bin/systemctl")
        .args(["--user", "set-property", "--runtime", scope])
        .arg(format!("CPUQuota={percent}%"))
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "systemctl set-property failed with {status}"
        )))
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn mach_ticks_to_nanos(ticks: u64) -> u64 {
    use std::sync::OnceLock;
    static TIMEBASE: OnceLock<(u64, u64)> = OnceLock::new();
    let (numer, denom) = *TIMEBASE.get_or_init(|| {
        let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
        let result = unsafe { libc::mach_timebase_info(&mut timebase) };
        if result == 0 && timebase.denom != 0 {
            (u64::from(timebase.numer), u64::from(timebase.denom))
        } else {
            (1, 1)
        }
    });
    ticks.saturating_mul(numer) / denom
}
