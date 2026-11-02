package main

import (
	"database/sql"
	"os"
	"syscall"
	"time"
)

type taskHistoryInstance struct {
	Attempt    int    `json:"attempt"`
	PID        int    `json:"pid"`
	PGID       int    `json:"pgid"`
	StartToken string `json:"start_token"`
	State      string `json:"state"`
	ExitCode   *int   `json:"exit_code"`
	StartedAt  int64  `json:"started_at"`
	EndedAt    *int64 `json:"ended_at"`
	DurationMS *int64 `json:"duration_ms"`
	LogPath    string `json:"log_path"`
}

type taskHistoryResponse struct {
	OK        bool                  `json:"ok"`
	RequestID string                `json:"request_id"`
	TaskID    string                `json:"task_id,omitempty"`
	Attempts  int                   `json:"attempts"`
	Instances []taskHistoryInstance `json:"instances"`
	Error     string                `json:"error,omitempty"`
	Message   string                `json:"message,omitempty"`
}

func registerTaskInstance(database *sql.DB, task *taskRecord) error {
	if task.State != "running" {
		return nil
	}
	_, err := database.Exec(
		`INSERT INTO task_instances
		 (id, task_key, attempt, pid, pgid, start_token, state, exit_code,
		  log_path, started_at)
		 VALUES (?, ?, 1, ?, ?, ?, 'running', NULL, ?, ?)`,
		task.ID+"-instance-1",
		task.ID,
		task.PID,
		task.PGID,
		task.StartToken,
		task.LogPath,
		time.Now().UnixMilli(),
	)
	return err
}

func finishTaskInstance(database *sql.DB, taskID string, exitCode int) {
	now := time.Now().UnixMilli()
	instanceState := "exited"
	if exitCode != 0 {
		instanceState = "failed"
	}
	_, _ = database.Exec(
		`UPDATE task_instances
		 SET state = ?, exit_code = ?, ended_at = ?
		 WHERE task_key = ? AND attempt = 1`,
		instanceState,
		exitCode,
		now,
		taskID,
	)
	_, _ = transitionTaskState(database, taskID, "running", instanceState)
}

func taskHistory(
	database *sql.DB,
	requestID, taskID string,
) (taskHistoryResponse, error) {
	var exists int
	if err := database.QueryRow(
		"SELECT COUNT(*) FROM tasks WHERE id = ?",
		taskID,
	).Scan(&exists); err != nil {
		return taskHistoryResponse{}, err
	}
	if exists == 0 {
		return taskHistoryResponse{
			OK:        false,
			RequestID: requestID,
			Error:     "not_found",
			Message:   "task not found: " + taskID,
		}, nil
	}
	rows, err := database.Query(
		`SELECT attempt, pid, pgid, start_token, state, exit_code,
		        started_at, ended_at, log_path
		 FROM task_instances WHERE task_key = ? ORDER BY attempt`,
		taskID,
	)
	if err != nil {
		return taskHistoryResponse{}, err
	}
	defer rows.Close()
	var instances []taskHistoryInstance
	for rows.Next() {
		var instance taskHistoryInstance
		var exitCode sql.NullInt64
		var endedAt sql.NullInt64
		if err := rows.Scan(
			&instance.Attempt,
			&instance.PID,
			&instance.PGID,
			&instance.StartToken,
			&instance.State,
			&exitCode,
			&instance.StartedAt,
			&endedAt,
			&instance.LogPath,
		); err != nil {
			return taskHistoryResponse{}, err
		}
		if exitCode.Valid {
			value := int(exitCode.Int64)
			instance.ExitCode = &value
		}
		if endedAt.Valid {
			value := endedAt.Int64
			instance.EndedAt = &value
			duration := value - instance.StartedAt
			instance.DurationMS = &duration
		}
		instances = append(instances, instance)
	}
	if err := rows.Err(); err != nil {
		return taskHistoryResponse{}, err
	}
	if instances == nil {
		instances = []taskHistoryInstance{}
	}
	return taskHistoryResponse{
		OK:        true,
		RequestID: requestID,
		TaskID:    taskID,
		Attempts:  len(instances),
		Instances: instances,
	}, nil
}

func signalTask(
	database *sql.DB,
	requestID, taskID, signalName string,
) runtimeResponse {
	task, err := loadTaskByID(database, taskID)
	if err != nil {
		return errorJSON(requestID, "internal", err.Error())
	}
	if task == nil {
		return errorJSON(requestID, "not_found", "task not found: "+taskID)
	}
	if task.State != "running" || !taskAlive(task) {
		return errorJSON(requestID, "not_running", "task is not running: "+taskID)
	}
	var number syscall.Signal
	switch signalName {
	case "SIGINT":
		number = syscall.SIGINT
	case "SIGTERM":
		number = syscall.SIGTERM
	case "SIGKILL":
		number = syscall.SIGKILL
	default:
		return errorJSON(requestID, "invalid_request",
			"signal must be SIGINT, SIGTERM, or SIGKILL")
	}
	if err := syscall.Kill(-task.PGID, number); err != nil {
		return errorJSON(requestID, "internal", err.Error())
	}
	return taskJSON(requestID, task, nil)
}

func processExitCode(processState *os.ProcessState) int {
	if processState == nil {
		return 1
	}
	if status, ok := processState.Sys().(syscall.WaitStatus); ok {
		if status.Signaled() {
			return 128 + int(status.Signal())
		}
		return status.ExitStatus()
	}
	return processState.ExitCode()
}
