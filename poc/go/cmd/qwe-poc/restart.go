package main

import (
	"database/sql"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"syscall"
	"time"
)

type restartTaskRecord struct {
	Key         string
	Policy      string
	MaxAttempts int
	LogPrefix   string
	Command     string
	State       string
}

type restartInstance struct {
	Attempt    int    `json:"attempt"`
	PID        int    `json:"pid"`
	PGID       int    `json:"pgid"`
	StartToken string `json:"start_token"`
	State      string `json:"state"`
	ExitCode   *int   `json:"exit_code"`
	LogPath    string `json:"log_path"`
}

type restartResponse struct {
	OK        bool              `json:"ok"`
	RequestID string            `json:"request_id"`
	TaskKey   string            `json:"task_key"`
	Policy    string            `json:"policy"`
	State     string            `json:"state"`
	Created   *bool             `json:"created,omitempty"`
	Attempts  int               `json:"attempts"`
	Instances []restartInstance `json:"instances"`
	Error     string            `json:"error,omitempty"`
	Message   string            `json:"message,omitempty"`
}

func restartError(requestID, code, message string) restartResponse {
	return restartResponse{
		OK:        false,
		RequestID: requestID,
		Error:     code,
		Message:   message,
	}
}

func loadRestartTask(database *sql.DB, key string) (*restartTaskRecord, error) {
	var task restartTaskRecord
	err := database.QueryRow(
		`SELECT task_key, policy, max_attempts, log_prefix, command, state
		 FROM restart_tasks WHERE task_key = ?`,
		key,
	).Scan(
		&task.Key,
		&task.Policy,
		&task.MaxAttempts,
		&task.LogPrefix,
		&task.Command,
		&task.State,
	)
	if err == sql.ErrNoRows {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &task, nil
}

func restartStatus(
	database *sql.DB,
	requestID, key string,
	created *bool,
) (restartResponse, error) {
	task, err := loadRestartTask(database, key)
	if err != nil {
		return restartResponse{}, err
	}
	if task == nil {
		return restartError(requestID, "not_found",
			"restart task not found: "+key), nil
	}
	rows, err := database.Query(
		`SELECT attempt, pid, pgid, start_token, state, exit_code, log_path
		 FROM task_instances WHERE task_key = ? ORDER BY attempt`,
		key,
	)
	if err != nil {
		return restartResponse{}, err
	}
	defer rows.Close()
	var instances []restartInstance
	for rows.Next() {
		var instance restartInstance
		var exitCode sql.NullInt64
		if err := rows.Scan(
			&instance.Attempt,
			&instance.PID,
			&instance.PGID,
			&instance.StartToken,
			&instance.State,
			&exitCode,
			&instance.LogPath,
		); err != nil {
			return restartResponse{}, err
		}
		if exitCode.Valid {
			value := int(exitCode.Int64)
			instance.ExitCode = &value
		}
		instances = append(instances, instance)
	}
	if err := rows.Err(); err != nil {
		return restartResponse{}, err
	}
	if instances == nil {
		instances = []restartInstance{}
	}
	return restartResponse{
		OK:        true,
		RequestID: requestID,
		TaskKey:   task.Key,
		Policy:    task.Policy,
		State:     task.State,
		Created:   created,
		Attempts:  len(instances),
		Instances: instances,
	}, nil
}

func startRestartTask(
	database *sql.DB,
	requestID, key, logPrefix, policy, maxAttemptsText, program string,
	arguments []string,
) restartResponse {
	if policy != "never" && policy != "on-failure" && policy != "always" {
		return restartError(requestID, "invalid_request",
			"policy must be never, on-failure, or always")
	}
	maxAttempts, err := strconv.Atoi(maxAttemptsText)
	if err != nil || maxAttempts < 1 || maxAttempts > 20 {
		return restartError(requestID, "invalid_request",
			"max attempts must be between 1 and 20")
	}
	command := strings.Join(append([]string{program}, arguments...), " ")
	now := time.Now().Unix()
	_, err = database.Exec(
		`INSERT INTO restart_tasks
		 (task_key, policy, max_attempts, log_prefix, command, state,
		  created_at, updated_at)
		 VALUES (?, ?, ?, ?, ?, 'running', ?, ?)`,
		key,
		policy,
		maxAttempts,
		logPrefix,
		command,
		now,
		now,
	)
	created := err == nil
	if err != nil && !strings.Contains(err.Error(), "UNIQUE constraint failed") {
		return restartError(requestID, "internal", err.Error())
	}
	response, err := restartStatus(database, requestID, key, &created)
	if err != nil {
		return restartError(requestID, "internal", err.Error())
	}
	if created {
		task := restartTaskRecord{
			Key:         key,
			Policy:      policy,
			MaxAttempts: maxAttempts,
			LogPrefix:   logPrefix,
			Command:     command,
			State:       "running",
		}
		go runRestartSequence(database, task, program, arguments)
	}
	return response
}

func runRestartSequence(
	database *sql.DB,
	task restartTaskRecord,
	program string,
	arguments []string,
) {
	failed := false
	for attempt := 1; attempt <= task.MaxAttempts; attempt++ {
		exitCode, err := runRestartAttempt(
			database, task, attempt, program, arguments,
		)
		if err != nil {
			failed = true
			break
		}
		restart := false
		switch task.Policy {
		case "on-failure":
			restart = exitCode != 0
		case "always":
			restart = true
		}
		if !restart {
			break
		}
	}
	finalState := "completed"
	if failed {
		finalState = "failed"
	}
	_, _ = database.Exec(
		"UPDATE restart_tasks SET state = ?, updated_at = ? WHERE task_key = ?",
		finalState,
		time.Now().Unix(),
		task.Key,
	)
}

func runRestartAttempt(
	database *sql.DB,
	task restartTaskRecord,
	attempt int,
	program string,
	arguments []string,
) (int, error) {
	logPath := fmt.Sprintf("%s.%d.log", task.LogPrefix, attempt)
	logFile, err := os.OpenFile(
		logPath,
		os.O_CREATE|os.O_APPEND|os.O_WRONLY,
		0o644,
	)
	if err != nil {
		return 0, err
	}
	defer logFile.Close()
	command := exec.Command(program, arguments...)
	command.Stdin = nil
	command.Stdout = logFile
	command.Stderr = logFile
	command.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := command.Start(); err != nil {
		return 0, err
	}
	pid := command.Process.Pid
	startedAt := time.Now().Unix()
	startToken := ""
	for retry := 0; retry < 10; retry++ {
		time.Sleep(10 * time.Millisecond)
		token, tokenErr := processStartToken(pid)
		if tokenErr == nil {
			startToken = token
			break
		}
		if !processAlive(pid) {
			break
		}
	}
	if startToken == "" && processAlive(pid) {
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		_ = command.Wait()
		return 0, fmt.Errorf("restart instance identity unavailable")
	}
	instanceID := fmt.Sprintf("%s-%d-%d", task.Key, attempt, time.Now().UnixNano())
	_, err = database.Exec(
		`INSERT INTO task_instances
		 (id, task_key, attempt, pid, pgid, start_token, state, exit_code,
		  log_path, started_at)
		 VALUES (?, ?, ?, ?, ?, ?, 'running', NULL, ?, ?)`,
		instanceID,
		task.Key,
		attempt,
		pid,
		pid,
		startToken,
		logPath,
		startedAt,
	)
	if err != nil {
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		_ = command.Wait()
		return 0, err
	}
	waitErr := command.Wait()
	exitCode := 0
	if waitErr != nil {
		exitCode = 1
		if exitError, ok := waitErr.(*exec.ExitError); ok {
			if status, ok := exitError.Sys().(syscall.WaitStatus); ok {
				if status.Signaled() {
					exitCode = 128 + int(status.Signal())
				} else {
					exitCode = status.ExitStatus()
				}
			}
		}
	}
	_, err = database.Exec(
		`UPDATE task_instances
		 SET state = 'exited', exit_code = ?, ended_at = ?
		 WHERE id = ?`,
		exitCode,
		time.Now().Unix(),
		instanceID,
	)
	return exitCode, err
}
