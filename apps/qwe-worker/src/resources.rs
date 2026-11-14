use qwe_core::ResourcePolicy;
use serde_json::{Value, json};
use std::collections::VecDeque;
#[cfg(target_os = "linux")]
use std::ffi::CString;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(100);
const RESOURCE_EVENT_LIMIT: usize = 32;

#[derive(Debug, Clone)]
pub struct ResourceEvent {
    pub at_ms: u64,
    pub kind: String,
    pub severity: String,
    pub message: String,
}

impl ResourceEvent {
    fn to_json(&self) -> Value {
        json!({
            "at_ms": self.at_ms,
            "kind": self.kind,
            "severity": self.severity,
            "message": self.message
        })
    }
}

#[derive(Debug, Clone)]
pub struct ResourceMetrics {
    pub memory_current_bytes: u64,
    pub memory_peak_bytes: u64,
    pub tasks_current: u64,
    pub tasks_peak: u64,
    pub cpu_microseconds: u64,
    pub sampled_at_ms: u64,
    pub last_violation: Option<String>,
    pub stats: qwe_runtime::CgroupStats,
    pub events: VecDeque<ResourceEvent>,
    #[cfg(target_os = "linux")]
    memory_event_max_seen: u64,
    #[cfg(target_os = "linux")]
    memory_oom_seen: u64,
    #[cfg(target_os = "linux")]
    memory_oom_kill_seen: u64,
    #[cfg(target_os = "linux")]
    pids_event_max_seen: u64,
    #[cfg(target_os = "linux")]
    cpu_throttled_seen: u64,
}

impl Default for ResourceMetrics {
    fn default() -> Self {
        Self {
            memory_current_bytes: 0,
            memory_peak_bytes: 0,
            tasks_current: 0,
            tasks_peak: 0,
            cpu_microseconds: 0,
            sampled_at_ms: 0,
            last_violation: None,
            stats: qwe_runtime::CgroupStats::default(),
            events: VecDeque::with_capacity(RESOURCE_EVENT_LIMIT),
            #[cfg(target_os = "linux")]
            memory_event_max_seen: 0,
            #[cfg(target_os = "linux")]
            memory_oom_seen: 0,
            #[cfg(target_os = "linux")]
            memory_oom_kill_seen: 0,
            #[cfg(target_os = "linux")]
            pids_event_max_seen: 0,
            #[cfg(target_os = "linux")]
            cpu_throttled_seen: 0,
        }
    }
}

impl ResourceMetrics {
    #[must_use]
    pub fn json(&self, tier: &str, policy: &ResourcePolicy) -> Value {
        let health = if self.last_violation.is_some() {
            "enforced"
        } else if is_near_limit(self.memory_current_bytes, policy.memory_max_bytes)
            || is_near_limit(self.memory_current_bytes, policy.memory_high_bytes)
            || is_near_limit(self.tasks_current, policy.tasks_max)
        {
            "near_limit"
        } else if self
            .stats
            .memory_pressure
            .as_ref()
            .is_some_and(|pressure| pressure.avg10 >= 10.0)
            || self
                .stats
                .cpu_pressure
                .as_ref()
                .is_some_and(|pressure| pressure.avg10 >= 10.0)
        {
            "pressure"
        } else {
            "healthy"
        };
        json!({
            "tier": tier,
            "health": health,
            "memory_current_bytes": self.memory_current_bytes,
            "memory_peak_bytes": self.memory_peak_bytes,
            "tasks_current": self.tasks_current,
            "tasks_peak": self.tasks_peak,
            "cpu_seconds": self.cpu_microseconds as f64 / 1_000_000.0,
            "sampled_at_ms": self.sampled_at_ms,
            "last_violation": self.last_violation,
            "limits": {
                "memory_high_bytes": policy.memory_high_bytes,
                "memory_max_bytes": policy.memory_max_bytes,
                "tasks_max": policy.tasks_max,
                "cpu_quota_percent": policy.cpu_quota_percent,
                "cpu_seconds": policy.cpu_seconds
            },
            "remaining": {
                "memory_max_bytes": policy.memory_max_bytes.map(|limit| limit.saturating_sub(self.memory_current_bytes)),
                "tasks_max": policy.tasks_max.map(|limit| limit.saturating_sub(self.tasks_current))
            },
            "memory_events": counters_json(&self.stats.memory_events),
            "pids_events": counters_json(&self.stats.pids_events),
            "cpu_stat": counters_json(&self.stats.cpu_stat),
            "memory_pressure": pressure_json(self.stats.memory_pressure.as_ref()),
            "cpu_pressure": pressure_json(self.stats.cpu_pressure.as_ref()),
            "events": self.events.iter().map(ResourceEvent::to_json).collect::<Vec<_>>(),
            "event_retention": {
                "scope": "pane_worker",
                "maximum_events": RESOURCE_EVENT_LIMIT,
                "durable": false,
                "policy": "latest events retained in memory until worker replacement"
            }
        })
    }
}

fn is_near_limit(current: u64, limit: Option<u64>) -> bool {
    limit.is_some_and(|limit| limit > 0 && current.saturating_mul(100) >= limit.saturating_mul(90))
}

fn counters_json(counters: &[(String, u64)]) -> Value {
    Value::Object(
        counters
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect(),
    )
}

fn pressure_json(pressure: Option<&qwe_runtime::Pressure>) -> Value {
    pressure.map_or(Value::Null, |pressure| {
        json!({
            "avg10": pressure.avg10,
            "avg60": pressure.avg60,
            "avg300": pressure.avg300,
            "total_us": pressure.total_us
        })
    })
}

#[derive(Clone)]
pub struct ResourceController {
    policy: Arc<Mutex<ResourcePolicy>>,
    effective_policy: Arc<Mutex<Option<ResourcePolicy>>>,
    metrics: Arc<Mutex<ResourceMetrics>>,
    final_sample_taken: Arc<AtomicBool>,
    #[cfg(target_os = "linux")]
    event_watcher_started: Arc<AtomicBool>,
    #[cfg(target_os = "linux")]
    event_watcher_stop: Arc<AtomicBool>,
    cgroup_dir: Option<PathBuf>,
}

impl ResourceController {
    pub fn new(policy: ResourcePolicy, cgroup_dir: Option<PathBuf>) -> Self {
        Self {
            policy: Arc::new(Mutex::new(policy.clone())),
            effective_policy: Arc::new(Mutex::new(Some(policy))),
            metrics: Arc::new(Mutex::new(ResourceMetrics::default())),
            final_sample_taken: Arc::new(AtomicBool::new(false)),
            #[cfg(target_os = "linux")]
            event_watcher_started: Arc::new(AtomicBool::new(false)),
            #[cfg(target_os = "linux")]
            event_watcher_stop: Arc::new(AtomicBool::new(false)),
            cgroup_dir,
        }
    }

    pub fn update(&self, policy: ResourcePolicy) -> std::io::Result<()> {
        if let Some(cgroup) = &self.cgroup_dir {
            let previous = qwe_runtime::read_effective_resource_policy(cgroup).ok();
            let apply = || {
                let result = qwe_runtime::apply_resource_policy(cgroup, &policy);
                if result
                    .as_ref()
                    .is_err_and(|failure| failure.kind() == std::io::ErrorKind::NotFound)
                    && policy.cpu_quota_percent.is_some()
                {
                    enable_cpu_quota(cgroup, policy.cpu_quota_percent.unwrap_or_default())?;
                    qwe_runtime::apply_resource_policy(cgroup, &policy)
                } else {
                    result
                }
            };
            if let Err(failure) = apply().map_err(|failure| {
                std::io::Error::new(failure.kind(), format!("{}: {failure}", cgroup.display()))
            }) {
                if let Some(previous) = previous.as_ref() {
                    let _ = qwe_runtime::apply_resource_policy(cgroup, previous);
                }
                return Err(failure);
            }
        }
        *self
            .policy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = policy;
        let policy = self
            .policy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(cgroup) = &self.cgroup_dir {
            if let Ok(effective) = qwe_runtime::read_effective_resource_policy(cgroup) {
                *self
                    .effective_policy
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(effective);
            }
        }
        record_event(
            &mut self
                .metrics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            "policy_update",
            "info",
            format!("resource policy updated: {policy:?}"),
        );
        Ok(())
    }

    pub fn update_requested(&self, policy: ResourcePolicy) {
        *self
            .policy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = policy.clone();
        record_event(
            &mut self
                .metrics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            "policy_update",
            "info",
            format!("requested resource policy updated for exited Pane: {policy:?}"),
        );
    }

    #[must_use]
    pub fn metrics_json(&self) -> Value {
        let policy = self
            .policy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let metrics = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        metrics.json(
            if self.cgroup_dir.is_some() {
                "hard"
            } else {
                "monitored"
            },
            &policy,
        )
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

    #[must_use]
    pub fn effective_policy_json(&self) -> Value {
        let policy = self
            .cgroup_dir
            .as_ref()
            .and_then(|cgroup| qwe_runtime::read_effective_resource_policy(cgroup).ok())
            .or_else(|| {
                self.effective_policy
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
            })
            .unwrap_or_else(|| {
                self.policy
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
            });
        serde_json::to_value(policy).unwrap_or(Value::Null)
    }

    pub fn start_monitor(&self, pid: u32, pgid: u32) {
        let controller = self.clone();
        thread::spawn(move || controller.monitor(pid, pgid));
    }

    pub fn start_event_watcher(&self) -> std::io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            let Some(cgroup) = self.cgroup_dir.clone() else {
                return Ok(());
            };
            if self.event_watcher_started.swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            let metrics = Arc::clone(&self.metrics);
            let stop = Arc::clone(&self.event_watcher_stop);
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            thread::spawn(move || watch_cgroup_events(cgroup, metrics, stop, ready_tx));
            let ready = ready_rx.recv_timeout(Duration::from_secs(1)).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "cgroup event watcher did not become ready",
                )
            })?;
            if !ready {
                return Err(std::io::Error::other(
                    "failed to install cgroup event watcher",
                ));
            }
        }
        Ok(())
    }

    pub fn cleanup_cgroup(&self) {
        #[cfg(target_os = "linux")]
        self.event_watcher_stop.store(true, Ordering::SeqCst);
        if let Some(cgroup) = &self.cgroup_dir {
            if let Some(scope) = cgroup
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| value.ends_with(".scope"))
            {
                let _ = qwe_runtime::stop_systemd_scope(scope);
            }
            let _ = std::fs::remove_dir(cgroup);
        }
    }

    pub fn capture_final_sample(&self) {
        #[cfg(target_os = "linux")]
        let _ = self.sample_linux();
        self.final_sample_taken.store(true, Ordering::SeqCst);
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
                record_event(
                    &mut self
                        .metrics
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                    &reason,
                    "hard",
                    format!("Pane terminated after reaching {reason}"),
                );
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
        for _ in 0..50 {
            if self.final_sample_taken.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.cleanup_cgroup();
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
        if let Ok(stats) = qwe_runtime::read_cgroup_stats(cgroup) {
            observe_stats(&mut metrics, &stats);
            metrics.stats = stats;
        }
        if let Ok(Some(event)) = qwe_runtime::read_enforcement_event(cgroup) {
            if metrics.last_violation.as_ref() != Some(&event) {
                record_event(
                    &mut metrics,
                    &event,
                    "hard",
                    format!("kernel enforcement event: {event}"),
                );
                metrics.last_violation.get_or_insert(event);
            }
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
        if self.cgroup_dir.is_some() {
            return policy
                .cpu_seconds
                .is_some_and(|maximum| metrics.cpu_microseconds > maximum.saturating_mul(1_000_000))
                .then(|| "cpu_seconds".to_string());
        }
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

#[cfg(target_os = "linux")]
fn watch_cgroup_events(
    cgroup: PathBuf,
    metrics: Arc<Mutex<ResourceMetrics>>,
    stop: Arc<AtomicBool>,
    ready: std::sync::mpsc::Sender<bool>,
) {
    const IN_MODIFY: u32 = 0x0000_0002;
    const IN_DELETE_SELF: u32 = 0x0000_0400;
    const IN_MOVE_SELF: u32 = 0x0000_0800;
    const IN_IGNORED: u32 = 0x0000_8000;

    let descriptor = unsafe { libc::inotify_init1(libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if descriptor < 0 {
        let _ = ready.send(false);
        return;
    }
    let Ok(path) = CString::new(cgroup.as_os_str().as_bytes()) else {
        unsafe {
            libc::close(descriptor);
        }
        let _ = ready.send(false);
        return;
    };
    let watch = unsafe {
        libc::inotify_add_watch(
            descriptor,
            path.as_ptr(),
            IN_MODIFY | IN_DELETE_SELF | IN_MOVE_SELF,
        )
    };
    if watch < 0 {
        unsafe {
            libc::close(descriptor);
        }
        let _ = ready.send(false);
        return;
    }
    let _ = ready.send(true);

    while !stop.load(Ordering::SeqCst) {
        let mut poll = libc::pollfd {
            fd: descriptor,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, 100) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if ready == 0 || poll.revents & libc::POLLIN == 0 {
            continue;
        }
        let mut buffer = [0_u8; 4096];
        let count = unsafe { libc::read(descriptor, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count <= 0 {
            if count < 0
                && matches!(
                    std::io::Error::last_os_error().kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                )
            {
                continue;
            }
            break;
        }
        let mut terminal_event = false;
        let mut offset = 0;
        while offset + std::mem::size_of::<libc::inotify_event>() <= count as usize {
            let event = unsafe {
                buffer
                    .as_ptr()
                    .add(offset)
                    .cast::<libc::inotify_event>()
                    .read_unaligned()
            };
            if event.mask & (IN_DELETE_SELF | IN_MOVE_SELF | IN_IGNORED) != 0 {
                terminal_event = true;
            }
            offset += std::mem::size_of::<libc::inotify_event>() + event.len as usize;
        }
        let usage = qwe_runtime::read_resource_usage(&cgroup).ok();
        let cpu_microseconds = qwe_runtime::read_cpu_usage_microseconds(&cgroup).ok();
        let stats = qwe_runtime::read_cgroup_stats(&cgroup).ok();
        if usage.is_some() || cpu_microseconds.is_some() || stats.is_some() {
            let mut metrics = metrics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((memory_current, memory_peak, tasks_current, tasks_peak)) = usage {
                metrics.memory_current_bytes = memory_current;
                metrics.memory_peak_bytes = metrics.memory_peak_bytes.max(memory_peak);
                metrics.tasks_current = tasks_current;
                metrics.tasks_peak = metrics.tasks_peak.max(tasks_peak);
            }
            if let Some(cpu_microseconds) = cpu_microseconds {
                metrics.cpu_microseconds = cpu_microseconds;
            }
            if let Some(stats) = stats {
                observe_stats(&mut metrics, &stats);
                metrics.stats = stats;
            }
            metrics.sampled_at_ms = now_ms();
        }
        if terminal_event {
            break;
        }
    }
    unsafe {
        libc::close(descriptor);
    }
}

fn record_event(metrics: &mut ResourceMetrics, kind: &str, severity: &str, message: String) {
    while metrics.events.len() >= RESOURCE_EVENT_LIMIT {
        metrics.events.pop_front();
    }
    metrics.events.push_back(ResourceEvent {
        at_ms: now_ms(),
        kind: kind.to_string(),
        severity: severity.to_string(),
        message,
    });
}

#[cfg(target_os = "linux")]
fn counter_value(counters: &[(String, u64)], name: &str) -> u64 {
    counters
        .iter()
        .find_map(|(candidate, value)| (candidate == name).then_some(*value))
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn observe_stats(metrics: &mut ResourceMetrics, stats: &qwe_runtime::CgroupStats) {
    let memory_max = counter_value(&stats.memory_events, "max");
    if memory_max > metrics.memory_event_max_seen {
        record_event(
            metrics,
            "memory_max",
            "hard",
            format!("kernel memory.max event count increased to {memory_max}"),
        );
        metrics
            .last_violation
            .get_or_insert_with(|| "memory_max".into());
    }
    metrics.memory_event_max_seen = memory_max;

    let memory_oom = counter_value(&stats.memory_events, "oom");
    let memory_oom_kill = counter_value(&stats.memory_events, "oom_kill");
    if memory_oom > metrics.memory_oom_seen || memory_oom_kill > metrics.memory_oom_kill_seen {
        record_event(
            metrics,
            "memory_max",
            "hard",
            format!("kernel OOM events increased (oom={memory_oom}, oom_kill={memory_oom_kill})"),
        );
        metrics
            .last_violation
            .get_or_insert_with(|| "memory_max".into());
    }
    metrics.memory_oom_seen = memory_oom;
    metrics.memory_oom_kill_seen = memory_oom_kill;

    let pids_max = counter_value(&stats.pids_events, "max");
    if pids_max > metrics.pids_event_max_seen {
        record_event(
            metrics,
            "tasks_max",
            "hard",
            format!("kernel pids.max event count increased to {pids_max}"),
        );
        metrics
            .last_violation
            .get_or_insert_with(|| "tasks_max".into());
    }
    metrics.pids_event_max_seen = pids_max;

    let throttled = counter_value(&stats.cpu_stat, "nr_throttled");
    if throttled > metrics.cpu_throttled_seen {
        record_event(
            metrics,
            "cpu_throttled",
            "warning",
            format!("CPU quota throttled the Pane {throttled} times"),
        );
    }
    metrics.cpu_throttled_seen = throttled;
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
