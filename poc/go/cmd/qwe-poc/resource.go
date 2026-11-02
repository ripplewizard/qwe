package main

import (
	"database/sql"
	"os/exec"
	"strconv"
	"strings"
	"time"
)

type resourceSample struct {
	ProcessCount int     `json:"process_count"`
	PIDs         []int   `json:"pids"`
	CPUPercent   float64 `json:"cpu_percent"`
	MemoryBytes  int64   `json:"memory_bytes"`
}

type gpuDevice struct {
	Index         int    `json:"index"`
	Name          string `json:"name"`
	Utilization   int    `json:"utilization_percent"`
	MemoryUsedMiB int64  `json:"memory_used_mib"`
}

type gpuProcess struct {
	PID           int   `json:"pid"`
	MemoryUsedMiB int64 `json:"memory_used_mib"`
}

type gpuInfo struct {
	Supported         bool         `json:"supported"`
	Reason            string       `json:"reason,omitempty"`
	Devices           []gpuDevice  `json:"devices"`
	Processes         []gpuProcess `json:"processes"`
	AttributedToTask  bool         `json:"attributed_to_task"`
	AttributionMethod string       `json:"attribution_method,omitempty"`
}

type resourceResponse struct {
	OK            bool    `json:"ok"`
	RequestID     string  `json:"request_id"`
	TaskID        string  `json:"task_id,omitempty"`
	ProcessCount  int     `json:"process_count"`
	PIDs          []int   `json:"pids"`
	CPUPercent    float64 `json:"cpu_percent"`
	CPUConvention string  `json:"cpu_convention"`
	MemoryBytes   int64   `json:"memory_bytes"`
	MemoryMethod  string  `json:"memory_method"`
	GPU           gpuInfo `json:"gpu"`
	Error         string  `json:"error,omitempty"`
	Message       string  `json:"message,omitempty"`
}

type resourceSamplesResponse struct {
	OK          bool             `json:"ok"`
	RequestID   string           `json:"request_id"`
	TaskID      string           `json:"task_id,omitempty"`
	IntervalMS  int              `json:"interval_ms"`
	SampleCount int              `json:"sample_count"`
	Samples     []resourceSample `json:"samples"`
	OverheadMS  []int64          `json:"overhead_ms"`
	Error       string           `json:"error,omitempty"`
	Message     string           `json:"message,omitempty"`
}

func collectProcessGroup(pgid int) (resourceSample, error) {
	output, err := exec.Command(
		"ps", "-eo", "pid=,pgid=,pcpu=,rss=",
	).Output()
	if err != nil {
		return resourceSample{}, err
	}
	sample := resourceSample{PIDs: []int{}}
	for _, line := range strings.Split(string(output), "\n") {
		fields := strings.Fields(line)
		if len(fields) != 4 {
			continue
		}
		pid, pidErr := strconv.Atoi(fields[0])
		rowPGID, pgidErr := strconv.Atoi(fields[1])
		cpu, cpuErr := strconv.ParseFloat(fields[2], 64)
		rssKiB, rssErr := strconv.ParseInt(fields[3], 10, 64)
		if pidErr != nil || pgidErr != nil || cpuErr != nil ||
			rssErr != nil || rowPGID != pgid {
			continue
		}
		sample.PIDs = append(sample.PIDs, pid)
		sample.CPUPercent += cpu
		sample.MemoryBytes += rssKiB * 1024
	}
	sample.ProcessCount = len(sample.PIDs)
	return sample, nil
}

func collectGPU(taskPIDs []int) gpuInfo {
	devicesOutput, err := exec.Command(
		"nvidia-smi",
		"--query-gpu=index,name,utilization.gpu,memory.used",
		"--format=csv,noheader,nounits",
	).Output()
	if err != nil {
		return gpuInfo{
			Supported: false,
			Reason:    "nvidia-smi unavailable or no supported GPU",
			Devices:   []gpuDevice{},
			Processes: []gpuProcess{},
		}
	}
	devices := []gpuDevice{}
	for _, line := range strings.Split(strings.TrimSpace(string(devicesOutput)), "\n") {
		fields := strings.Split(line, ",")
		if len(fields) != 4 {
			continue
		}
		index, _ := strconv.Atoi(strings.TrimSpace(fields[0]))
		utilization, _ := strconv.Atoi(strings.TrimSpace(fields[2]))
		memory, _ := strconv.ParseInt(strings.TrimSpace(fields[3]), 10, 64)
		devices = append(devices, gpuDevice{
			Index:         index,
			Name:          strings.TrimSpace(fields[1]),
			Utilization:   utilization,
			MemoryUsedMiB: memory,
		})
	}
	processesOutput, _ := exec.Command(
		"nvidia-smi",
		"--query-compute-apps=pid,used_memory",
		"--format=csv,noheader,nounits",
	).Output()
	processes := []gpuProcess{}
	attributed := false
	for _, line := range strings.Split(strings.TrimSpace(string(processesOutput)), "\n") {
		fields := strings.Split(line, ",")
		if len(fields) != 2 {
			continue
		}
		pid, pidErr := strconv.Atoi(strings.TrimSpace(fields[0]))
		memory, memoryErr := strconv.ParseInt(strings.TrimSpace(fields[1]), 10, 64)
		if pidErr != nil || memoryErr != nil {
			continue
		}
		processes = append(processes, gpuProcess{PID: pid, MemoryUsedMiB: memory})
		for _, taskPID := range taskPIDs {
			if taskPID == pid {
				attributed = true
			}
		}
	}
	return gpuInfo{
		Supported:         true,
		Devices:           devices,
		Processes:         processes,
		AttributedToTask:  attributed,
		AttributionMethod: "process_pid_in_task_group",
	}
}

func taskResource(
	database *sql.DB,
	requestID, taskID string,
) resourceResponse {
	task, err := loadTaskByID(database, taskID)
	if err != nil {
		return resourceResponse{OK: false, RequestID: requestID,
			Error: "internal", Message: err.Error()}
	}
	if task == nil {
		return resourceResponse{OK: false, RequestID: requestID,
			Error: "not_found", Message: "task not found: " + taskID}
	}
	if task.State != "running" || !taskAlive(task) {
		return resourceResponse{OK: false, RequestID: requestID,
			Error: "not_running", Message: "task is not running: " + taskID}
	}
	sample, err := collectProcessGroup(task.PGID)
	if err != nil {
		return resourceResponse{OK: false, RequestID: requestID,
			Error: "internal", Message: err.Error()}
	}
	return resourceResponse{
		OK:            true,
		RequestID:     requestID,
		TaskID:        taskID,
		ProcessCount:  sample.ProcessCount,
		PIDs:          sample.PIDs,
		CPUPercent:    sample.CPUPercent,
		CPUConvention: "single_core_percent_sum",
		MemoryBytes:   sample.MemoryBytes,
		MemoryMethod:  "sum_rss_shared_pages_may_be_double_counted",
		GPU:           collectGPU(sample.PIDs),
	}
}

func taskResourceSamples(
	database *sql.DB,
	requestID, taskID, intervalText, countText string,
) resourceSamplesResponse {
	interval, err := strconv.Atoi(intervalText)
	if err != nil || interval < 100 || interval > 5000 {
		return resourceSamplesResponse{OK: false, RequestID: requestID,
			Error: "invalid_request", Message: "interval must be 100..5000 ms"}
	}
	count, err := strconv.Atoi(countText)
	if err != nil || count < 1 || count > 10 {
		return resourceSamplesResponse{OK: false, RequestID: requestID,
			Error: "invalid_request", Message: "samples must be 1..10"}
	}
	task, err := loadTaskByID(database, taskID)
	if err != nil || task == nil {
		return resourceSamplesResponse{OK: false, RequestID: requestID,
			Error: "not_found", Message: "task not found: " + taskID}
	}
	if task.State != "running" || !taskAlive(task) {
		return resourceSamplesResponse{OK: false, RequestID: requestID,
			Error: "not_running", Message: "task is not running: " + taskID}
	}
	samples := []resourceSample{}
	overhead := []int64{}
	for index := 0; index < count; index++ {
		if index > 0 {
			time.Sleep(time.Duration(interval) * time.Millisecond)
		}
		started := time.Now()
		sample, sampleErr := collectProcessGroup(task.PGID)
		if sampleErr != nil {
			return resourceSamplesResponse{OK: false, RequestID: requestID,
				Error: "internal", Message: sampleErr.Error()}
		}
		samples = append(samples, sample)
		overhead = append(overhead, time.Since(started).Milliseconds())
	}
	return resourceSamplesResponse{
		OK:          true,
		RequestID:   requestID,
		TaskID:      taskID,
		IntervalMS:  interval,
		SampleCount: count,
		Samples:     samples,
		OverheadMS:  overhead,
	}
}
