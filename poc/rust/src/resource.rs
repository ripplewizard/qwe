use crate::runtime::json_escape;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Default)]
struct ProcessSample {
    pids: Vec<i64>,
    cpu_percent: f64,
    memory_bytes: i64,
}

struct GpuSample {
    supported: bool,
    reason: String,
    devices: Vec<String>,
    processes: Vec<String>,
    attributed: bool,
}

fn process_group(pgid: i64) -> Result<ProcessSample, String> {
    let output = Command::new("ps")
        .args(["-eo", "pid=,pgid=,pcpu=,rss="])
        .output()
        .map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut sample = ProcessSample::default();
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 4 {
            continue;
        }
        let row_pgid = fields[1].parse::<i64>().unwrap_or_default();
        if row_pgid != pgid {
            continue;
        }
        if let Ok(pid) = fields[0].parse::<i64>() {
            sample.pids.push(pid);
        }
        sample.cpu_percent += fields[2].parse::<f64>().unwrap_or_default();
        sample.memory_bytes += fields[3].parse::<i64>().unwrap_or_default() * 1024;
    }
    Ok(sample)
}

fn gpu(task_pids: &[i64]) -> GpuSample {
    let devices_output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,name,utilization.gpu,memory.used",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let Ok(devices_output) = devices_output else {
        return GpuSample {
            supported: false,
            reason: "nvidia-smi unavailable or no supported GPU".to_string(),
            devices: Vec::new(),
            processes: Vec::new(),
            attributed: false,
        };
    };
    if !devices_output.status.success() {
        return GpuSample {
            supported: false,
            reason: "nvidia-smi unavailable or no supported GPU".to_string(),
            devices: Vec::new(),
            processes: Vec::new(),
            attributed: false,
        };
    }
    let devices = String::from_utf8_lossy(&devices_output.stdout)
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let process_output = Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=pid,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok();
    let mut processes = Vec::new();
    let mut attributed = false;
    if let Some(output) = process_output {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            processes.push(line.to_string());
            if let Some(pid) = line
                .split(',')
                .next()
                .and_then(|value| value.trim().parse::<i64>().ok())
            {
                attributed |= task_pids.contains(&pid);
            }
        }
    }
    GpuSample {
        supported: true,
        reason: String::new(),
        devices,
        processes,
        attributed,
    }
}

fn gpu_json(sample: &GpuSample) -> String {
    let devices = sample
        .devices
        .iter()
        .map(|value| format!("\"{}\"", json_escape(value)))
        .collect::<Vec<_>>()
        .join(",");
    let processes = sample
        .processes
        .iter()
        .map(|value| format!("\"{}\"", json_escape(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"supported":{},"reason":"{}","devices":[{}],"processes":[{}],"attributed_to_task":{},"attribution_method":"process_pid_in_task_group"}}"#,
        sample.supported,
        json_escape(&sample.reason),
        devices,
        processes,
        sample.attributed
    )
}

pub fn response(task_id: &str, pgid: i64, request_id: &str) -> Result<String, String> {
    let sample = process_group(pgid)?;
    let pids = sample
        .pids
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        r#"{{"ok":true,"request_id":"{}","task_id":"{}","process_count":{},"pids":[{}],"cpu_percent":{},"cpu_convention":"single_core_percent_sum","memory_bytes":{},"memory_method":"sum_rss_shared_pages_may_be_double_counted","gpu":{}}}"#,
        json_escape(request_id),
        json_escape(task_id),
        sample.pids.len(),
        pids,
        sample.cpu_percent,
        sample.memory_bytes,
        gpu_json(&gpu(&sample.pids))
    ))
}

pub fn sample(
    task_id: &str,
    pgid: i64,
    request_id: &str,
    interval_text: &str,
    count_text: &str,
) -> Result<String, String> {
    let interval = interval_text
        .parse::<u64>()
        .ok()
        .filter(|value| (100..=5000).contains(value))
        .ok_or_else(|| "interval must be 100..5000 ms".to_string())?;
    let count = count_text
        .parse::<usize>()
        .ok()
        .filter(|value| (1..=10).contains(value))
        .ok_or_else(|| "samples must be 1..10".to_string())?;
    let mut samples = Vec::new();
    let mut overhead = Vec::new();
    for index in 0..count {
        if index > 0 {
            thread::sleep(Duration::from_millis(interval));
        }
        let started = Instant::now();
        let sample = process_group(pgid)?;
        let pids = sample
            .pids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        samples.push(format!(
            r#"{{"process_count":{},"pids":[{}],"cpu_percent":{},"memory_bytes":{}}}"#,
            sample.pids.len(),
            pids,
            sample.cpu_percent,
            sample.memory_bytes
        ));
        overhead.push(started.elapsed().as_millis().to_string());
    }
    Ok(format!(
        r#"{{"ok":true,"request_id":"{}","task_id":"{}","interval_ms":{},"sample_count":{},"samples":[{}],"overhead_ms":[{}]}}"#,
        json_escape(request_id),
        json_escape(task_id),
        interval,
        count,
        samples.join(","),
        overhead.join(",")
    ))
}
