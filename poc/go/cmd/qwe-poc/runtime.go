package main

import (
	"bufio"
	"database/sql"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"time"

	_ "github.com/mattn/go-sqlite3"
)

type taskRecord struct {
	ID         string
	Key        string
	Command    string
	LogPath    string
	PID        int
	PGID       int
	StartToken string
	State      string
}

type runtimeResponse struct {
	OK         bool   `json:"ok"`
	RequestID  string `json:"request_id"`
	TaskID     string `json:"task_id,omitempty"`
	SessionID  string `json:"session_id,omitempty"`
	PID        int    `json:"pid,omitempty"`
	PGID       int    `json:"pgid,omitempty"`
	StartToken string `json:"start_token,omitempty"`
	State      string `json:"state,omitempty"`
	Created    *bool  `json:"created,omitempty"`
	Error      string `json:"error,omitempty"`
	Message    string `json:"message,omitempty"`
}

const currentSchemaVersion = 2

type requestError struct {
	code    string
	message string
}

func (e requestError) Error() string {
	return e.message
}

func openDB(path string) (*sql.DB, error) {
	database, err := sql.Open("sqlite3", path)
	if err != nil {
		return nil, err
	}
	database.SetMaxOpenConns(1)
	var version int
	if err := database.QueryRow("PRAGMA user_version").Scan(&version); err != nil {
		database.Close()
		return nil, err
	}
	if version > currentSchemaVersion {
		database.Close()
		return nil, fmt.Errorf(
			"database schema version %d is newer than supported version %d",
			version,
			currentSchemaVersion,
		)
	}
	if _, err := database.Exec(`
		PRAGMA journal_mode=WAL;
		CREATE TABLE IF NOT EXISTS tasks (
			id TEXT PRIMARY KEY,
			idempotency_key TEXT UNIQUE NOT NULL,
			command TEXT NOT NULL,
			log_path TEXT NOT NULL,
			pid INTEGER NOT NULL,
			pgid INTEGER NOT NULL,
			start_token TEXT NOT NULL,
			state TEXT NOT NULL,
			created_at TEXT NOT NULL,
			updated_at TEXT NOT NULL
		);
		CREATE TABLE IF NOT EXISTS restart_tasks (
			task_key TEXT PRIMARY KEY,
			policy TEXT NOT NULL,
			max_attempts INTEGER NOT NULL,
			log_prefix TEXT NOT NULL,
			command TEXT NOT NULL,
			state TEXT NOT NULL,
			created_at INTEGER NOT NULL,
			updated_at INTEGER NOT NULL
		);
		CREATE TABLE IF NOT EXISTS task_instances (
			id TEXT PRIMARY KEY,
			task_key TEXT NOT NULL,
			attempt INTEGER NOT NULL,
			pid INTEGER NOT NULL,
			pgid INTEGER NOT NULL,
			start_token TEXT NOT NULL,
			state TEXT NOT NULL,
			exit_code INTEGER,
			log_path TEXT NOT NULL,
			started_at INTEGER NOT NULL,
			ended_at INTEGER,
			UNIQUE(task_key, attempt)
		);
	`); err != nil {
		database.Close()
		return nil, err
	}
	if _, err := database.Exec(fmt.Sprintf(
		"PRAGMA user_version = %d",
		currentSchemaVersion,
	)); err != nil {
		database.Close()
		return nil, err
	}
	if err := database.QueryRow("PRAGMA user_version").Scan(&version); err != nil {
		database.Close()
		return nil, err
	}
	if version != currentSchemaVersion {
		database.Close()
		return nil, fmt.Errorf(
			"database schema migration left version %d, expected %d",
			version,
			currentSchemaVersion,
		)
	}
	return database, nil
}

func scanTask(scanner interface{ Scan(...any) error }) (taskRecord, error) {
	var task taskRecord
	err := scanner.Scan(
		&task.ID,
		&task.Key,
		&task.Command,
		&task.LogPath,
		&task.PID,
		&task.PGID,
		&task.StartToken,
		&task.State,
	)
	return task, err
}

const taskColumns = "id, idempotency_key, command, log_path, pid, pgid, start_token, state"

func loadTaskByKey(database *sql.DB, key string) (*taskRecord, error) {
	row := database.QueryRow(
		"SELECT "+taskColumns+" FROM tasks WHERE idempotency_key = ?",
		key,
	)
	task, err := scanTask(row)
	if err == sql.ErrNoRows {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &task, nil
}

func loadTaskByID(database *sql.DB, id string) (*taskRecord, error) {
	row := database.QueryRow("SELECT "+taskColumns+" FROM tasks WHERE id = ?", id)
	task, err := scanTask(row)
	if err == sql.ErrNoRows {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &task, nil
}

func updateTaskState(database *sql.DB, id, state string) error {
	_, err := database.Exec(
		"UPDATE tasks SET state = ?, updated_at = ? WHERE id = ?",
		state,
		time.Now().Unix(),
		id,
	)
	return err
}

func transitionTaskState(database *sql.DB, id, from, to string) (bool, error) {
	result, err := database.Exec(
		"UPDATE tasks SET state = ?, updated_at = ? WHERE id = ? AND state = ?",
		to,
		time.Now().Unix(),
		id,
		from,
	)
	if err != nil {
		return false, err
	}
	rows, err := result.RowsAffected()
	return rows == 1, err
}

func processStartToken(pid int) (string, error) {
	if runtime.GOOS == "linux" {
		data, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
		if err != nil {
			return "", err
		}
		text := string(data)
		end := strings.LastIndex(text, ")")
		if end < 0 {
			return "", fmt.Errorf("invalid process stat")
		}
		fields := strings.Fields(text[end+1:])
		if len(fields) <= 19 {
			return "", fmt.Errorf("missing process start time")
		}
		return fields[19], nil
	}

	output, err := exec.Command(
		"ps",
		"-o",
		"lstart=",
		"-p",
		strconv.Itoa(pid),
	).Output()
	if err != nil {
		return "", err
	}
	token := strings.TrimSpace(string(output))
	if token == "" {
		return "", fmt.Errorf("process not found")
	}
	return token, nil
}

func processAlive(pid int) bool {
	err := syscall.Kill(pid, 0)
	return err == nil || err == syscall.EPERM
}

func groupAlive(pgid int) bool {
	err := syscall.Kill(-pgid, 0)
	return err == nil || err == syscall.EPERM
}

func taskAlive(task *taskRecord) bool {
	if !processAlive(task.PID) {
		return false
	}
	token, err := processStartToken(task.PID)
	return err == nil && token == task.StartToken
}

func taskJSON(requestID string, task *taskRecord, created *bool) runtimeResponse {
	return runtimeResponse{
		OK:         true,
		RequestID:  requestID,
		TaskID:     task.ID,
		PID:        task.PID,
		PGID:       task.PGID,
		StartToken: task.StartToken,
		State:      task.State,
		Created:    created,
	}
}

func errorJSON(requestID, code, message string) runtimeResponse {
	return runtimeResponse{
		OK:        false,
		RequestID: requestID,
		Error:     code,
		Message:   message,
	}
}

func recoverTasks(database *sql.DB) error {
	rows, err := database.Query(
		"SELECT " + taskColumns + " FROM tasks WHERE state = 'running'",
	)
	if err != nil {
		return err
	}
	var tasks []taskRecord
	for rows.Next() {
		task, err := scanTask(rows)
		if err != nil {
			rows.Close()
			return err
		}
		tasks = append(tasks, task)
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return err
	}
	rows.Close()
	for _, task := range tasks {
		if !taskAlive(&task) {
			if err := updateTaskState(database, task.ID, "exited"); err != nil {
				return err
			}
		}
	}
	return nil
}

func startTask(
	database *sql.DB,
	key, logPath, program string,
	arguments []string,
) (*taskRecord, bool, error) {
	existing, err := loadTaskByKey(database, key)
	if err != nil {
		return nil, false, err
	}
	if existing != nil {
		if existing.State == "running" && taskAlive(existing) {
			return existing, false, nil
		}
		return nil, false, requestError{
			code:    "internal",
			message: "idempotency key already used by a non-running task",
		}
	}

	logFile, err := os.OpenFile(
		logPath,
		os.O_CREATE|os.O_APPEND|os.O_WRONLY,
		0o644,
	)
	if err != nil {
		return nil, false, err
	}
	defer logFile.Close()

	command := exec.Command(program, arguments...)
	command.Stdin = nil
	command.Stdout = logFile
	command.Stderr = logFile
	command.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := command.Start(); err != nil {
		return nil, false, err
	}
	pid := command.Process.Pid
	startToken := ""
	state := "running"
	for attempt := 0; attempt < 10; attempt++ {
		time.Sleep(10 * time.Millisecond)
		token, tokenErr := processStartToken(pid)
		if tokenErr == nil {
			startToken = token
			break
		}
		if !processAlive(pid) {
			state = "exited"
			break
		}
	}
	if state == "running" && startToken == "" {
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		return nil, false, fmt.Errorf(
			"task identity unavailable for a running process",
		)
	}

	task := &taskRecord{
		ID:         fmt.Sprintf("task-%d-%d", pid, time.Now().UnixNano()),
		Key:        key,
		Command:    strings.Join(append([]string{program}, arguments...), " "),
		LogPath:    logPath,
		PID:        pid,
		PGID:       pid,
		StartToken: startToken,
		State:      state,
	}
	_, err = database.Exec(
		`INSERT INTO tasks
		 (id, idempotency_key, command, log_path, pid, pgid, start_token,
		  state, created_at, updated_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		task.ID,
		task.Key,
		task.Command,
		task.LogPath,
		task.PID,
		task.PGID,
		task.StartToken,
		task.State,
		time.Now().Unix(),
		time.Now().Unix(),
	)
	if err != nil {
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		_ = command.Wait()
		return nil, false, err
	}
	if err := registerTaskInstance(database, task); err != nil {
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		_ = command.Wait()
		_ = updateTaskState(database, task.ID, "failed")
		return nil, false, err
	}
	go func() {
		_ = command.Wait()
		finishTaskInstance(database, task.ID, processExitCode(command.ProcessState))
	}()
	return task, true, nil
}

func stopTask(database *sql.DB, id string) (*taskRecord, error) {
	task, err := loadTaskByID(database, id)
	if err != nil {
		return nil, err
	}
	if task == nil {
		return nil, requestError{code: "not_found", message: "task not found: " + id}
	}
	if task.State != "running" {
		return task, nil
	}

	if groupAlive(task.PGID) {
		_ = syscall.Kill(-task.PGID, syscall.SIGTERM)
		deadline := time.Now().Add(2 * time.Second)
		for groupAlive(task.PGID) && time.Now().Before(deadline) {
			time.Sleep(50 * time.Millisecond)
		}
		if groupAlive(task.PGID) {
			_ = syscall.Kill(-task.PGID, syscall.SIGKILL)
		}
	}
	if err := updateTaskState(database, id, "stopped"); err != nil {
		return nil, err
	}
	task.State = "stopped"
	return task, nil
}

func startTaskWithTimeout(
	database *sql.DB,
	key, logPath, timeoutText, program string,
	arguments []string,
) (*taskRecord, bool, error) {
	timeoutMS, err := strconv.ParseInt(timeoutText, 10, 64)
	if err != nil || timeoutMS <= 0 {
		return nil, false, requestError{
			code:    "invalid_request",
			message: "timeout must be a positive integer",
		}
	}
	started := time.Now()
	task, created, err := startTask(database, key, logPath, program, arguments)
	if err != nil || !created || task.State != "running" {
		return task, created, err
	}
	remaining := time.Duration(timeoutMS)*time.Millisecond - time.Since(started)
	if remaining < 0 {
		remaining = 0
	}
	go enforceTaskTimeout(database, *task, remaining)
	return task, created, nil
}

func enforceTaskTimeout(database *sql.DB, task taskRecord, timeout time.Duration) {
	time.Sleep(timeout)
	current, err := loadTaskByID(database, task.ID)
	if err != nil || current == nil || current.State != "running" ||
		!taskAlive(current) {
		return
	}
	claimed, err := transitionTaskState(database, current.ID, "running", "timing_out")
	if err != nil || !claimed {
		return
	}
	_ = syscall.Kill(-current.PGID, syscall.SIGTERM)
	graceDeadline := time.Now().Add(200 * time.Millisecond)
	for groupAlive(current.PGID) && time.Now().Before(graceDeadline) {
		time.Sleep(20 * time.Millisecond)
	}
	if groupAlive(current.PGID) {
		_ = syscall.Kill(-current.PGID, syscall.SIGKILL)
	}
	for attempt := 0; attempt < 50 && groupAlive(current.PGID); attempt++ {
		time.Sleep(10 * time.Millisecond)
	}
	finalState := "timed_out"
	if groupAlive(current.PGID) {
		finalState = "timeout_failed"
	}
	_, _ = transitionTaskState(database, current.ID, "timing_out", finalState)
}

func handleRuntimeRequest(
	database *sql.DB,
	leases *leaseManager,
	sessions *sessionManager,
	line string,
) any {
	fields := strings.Split(strings.TrimSuffix(line, "\n"), "\t")
	requestID := "unknown"
	if len(fields) > 1 {
		requestID = fields[1]
	}
	switch {
	case len(fields) >= 5 && fields[0] == "START":
		task, created, err := startTask(
			database,
			fields[2],
			fields[3],
			fields[4],
			fields[5:],
		)
		if err != nil {
			return errorJSON(requestID, "internal", err.Error())
		}
		return taskJSON(requestID, task, &created)
	case len(fields) >= 6 && fields[0] == "TIMEOUT_START":
		task, created, err := startTaskWithTimeout(
			database,
			fields[2],
			fields[3],
			fields[4],
			fields[5],
			fields[6:],
		)
		if err != nil {
			code := "internal"
			if request, ok := err.(requestError); ok {
				code = request.code
			}
			return errorJSON(requestID, code, err.Error())
		}
		return taskJSON(requestID, task, &created)
	case len(fields) >= 7 && fields[0] == "RESTART_START":
		return startRestartTask(
			database,
			requestID,
			fields[2],
			fields[3],
			fields[4],
			fields[5],
			fields[6],
			fields[7:],
		)
	case len(fields) == 3 && fields[0] == "RESTART_STATUS":
		response, err := restartStatus(database, requestID, fields[2], nil)
		if err != nil {
			return restartError(requestID, "internal", err.Error())
		}
		return response
	case len(fields) == 3 && fields[0] == "STATUS":
		task, err := loadTaskByID(database, fields[2])
		if err != nil {
			return errorJSON(requestID, "internal", err.Error())
		}
		if task == nil {
			return errorJSON(requestID, "not_found", "task not found: "+fields[2])
		}
		if task.State == "running" && !taskAlive(task) {
			_ = updateTaskState(database, task.ID, "exited")
			task.State = "exited"
		}
		return taskJSON(requestID, task, nil)
	case len(fields) == 3 && fields[0] == "STOP":
		task, err := stopTask(database, fields[2])
		if err != nil {
			code := "internal"
			if request, ok := err.(requestError); ok {
				code = request.code
			}
			return errorJSON(requestID, code, err.Error())
		}
		return taskJSON(requestID, task, nil)
	case len(fields) == 3 && fields[0] == "TASK_HISTORY":
		response, err := taskHistory(database, requestID, fields[2])
		if err != nil {
			return errorJSON(requestID, "internal", err.Error())
		}
		return response
	case len(fields) == 4 && fields[0] == "TASK_SIGNAL":
		return signalTask(database, requestID, fields[2], fields[3])
	case len(fields) == 3 && fields[0] == "TASK_RESOURCE":
		return taskResource(database, requestID, fields[2])
	case len(fields) == 5 && fields[0] == "TASK_RESOURCE_SAMPLE":
		return taskResourceSamples(
			database,
			requestID,
			fields[2],
			fields[3],
			fields[4],
		)
	case len(fields) == 4 && fields[0] == "LEASE_CLAIM":
		return leases.claim(requestID, fields[2], fields[3])
	case len(fields) == 4 && fields[0] == "LEASE_TOUCH":
		return leases.touch(requestID, fields[2], fields[3])
	case len(fields) == 4 && fields[0] == "LEASE_RELEASE":
		return leases.release(requestID, fields[2], fields[3])
	case len(fields) == 3 && fields[0] == "LEASE_STATUS":
		return leases.status(requestID, fields[2])
	case len(fields) == 4 && fields[0] == "LEASE_TAKEOVER":
		return leases.takeover(requestID, fields[2], fields[3])
	case len(fields) >= 5 && fields[0] == "SESSION_START":
		return sessions.start(
			requestID,
			fields[2],
			fields[3],
			fields[4],
			fields[5:],
		)
	case len(fields) == 4 && fields[0] == "SESSION_ATTACH":
		return sessions.attach(requestID, fields[2], fields[3])
	case len(fields) == 4 && fields[0] == "SESSION_DETACH":
		return sessions.detach(requestID, fields[2], fields[3])
	case len(fields) == 5 && fields[0] == "SESSION_INPUT":
		return sessions.input(requestID, fields[2], fields[3], fields[4])
	case len(fields) == 3 && fields[0] == "SESSION_STATUS":
		return sessions.status(requestID, fields[2])
	default:
		return errorJSON(requestID, "invalid_request", "unsupported request")
	}
}

func runRuntimeDaemon(socketPath, databasePath string) error {
	database, err := openDB(databasePath)
	if err != nil {
		return err
	}
	defer database.Close()
	if err := recoverTasks(database); err != nil {
		return err
	}
	leases := newLeaseManager()
	sessions := newSessionManager(leases)
	if err := os.MkdirAll(filepath.Dir(socketPath), 0o755); err != nil {
		return err
	}
	_ = os.Remove(socketPath)
	listener, err := net.Listen("unix", socketPath)
	if err != nil {
		return err
	}
	defer listener.Close()

	for {
		connection, err := listener.Accept()
		if err != nil {
			return err
		}
		go func(client net.Conn) {
			defer client.Close()
			line, readErr := bufio.NewReader(client).ReadString('\n')
			var response any
			if readErr != nil {
				response = errorJSON("unknown", "invalid_request", readErr.Error())
			} else {
				response = handleRuntimeRequest(database, leases, sessions, line)
			}
			data, _ := json.Marshal(response)
			_, _ = client.Write(append(data, '\n'))
		}(connection)
	}
}

func runRuntimeClient(operation string) int {
	arguments := os.Args[2:]
	minimum := 2
	if operation == "START" {
		minimum = 4
	}
	if operation == "TIMEOUT_START" {
		minimum = 5
	}
	if operation == "RESTART_START" {
		minimum = 6
	}
	if operation == "TASK_SIGNAL" {
		minimum = 3
	}
	if operation == "TASK_RESOURCE_SAMPLE" {
		minimum = 4
	}
	if operation == "LEASE_CLAIM" ||
		operation == "LEASE_TOUCH" ||
		operation == "LEASE_RELEASE" ||
		operation == "LEASE_TAKEOVER" {
		minimum = 3
	}
	sessionStart := operation == "SESSION_START"
	if sessionStart {
		minimum = 4
	}
	if operation == "SESSION_ATTACH" ||
		operation == "SESSION_DETACH" {
		minimum = 3
	}
	if operation == "SESSION_INPUT" {
		minimum = 4
	}
	if len(arguments) < minimum {
		fmt.Fprintln(os.Stderr, "invalid runtime client arguments")
		return 2
	}
	if operation != "START" &&
		operation != "TIMEOUT_START" &&
		operation != "RESTART_START" &&
		operation != "TASK_SIGNAL" &&
		operation != "TASK_RESOURCE_SAMPLE" &&
		operation != "LEASE_CLAIM" &&
		operation != "LEASE_TOUCH" &&
		operation != "LEASE_RELEASE" &&
		operation != "LEASE_TAKEOVER" &&
		operation != "SESSION_ATTACH" &&
		operation != "SESSION_DETACH" &&
		operation != "SESSION_INPUT" &&
		!sessionStart &&
		len(arguments) != 2 {
		fmt.Fprintln(os.Stderr, "invalid runtime client arguments")
		return 2
	}
	if (operation == "LEASE_CLAIM" ||
		operation == "LEASE_TOUCH" ||
		operation == "LEASE_RELEASE" ||
		operation == "LEASE_TAKEOVER") &&
		len(arguments) != 3 {
		fmt.Fprintln(os.Stderr, "invalid runtime client arguments")
		return 2
	}
	if (operation == "SESSION_ATTACH" || operation == "SESSION_DETACH") &&
		len(arguments) != 3 {
		fmt.Fprintln(os.Stderr, "invalid runtime client arguments")
		return 2
	}
	if operation == "SESSION_INPUT" && len(arguments) != 4 {
		fmt.Fprintln(os.Stderr, "invalid runtime client arguments")
		return 2
	}
	if operation == "TASK_SIGNAL" && len(arguments) != 3 {
		fmt.Fprintln(os.Stderr, "invalid runtime client arguments")
		return 2
	}
	if operation == "TASK_RESOURCE_SAMPLE" && len(arguments) != 4 {
		fmt.Fprintln(os.Stderr, "invalid runtime client arguments")
		return 2
	}
	requestID := fmt.Sprintf("request-%d-%d", os.Getpid(), time.Now().UnixNano())
	fields := append([]string{operation, requestID}, arguments[1:]...)
	connection, err := net.Dial("unix", arguments[0])
	if err != nil {
		fmt.Fprintf(os.Stderr, "IPC failed: %v\n", err)
		return 1
	}
	defer connection.Close()
	if _, err := fmt.Fprintln(connection, strings.Join(fields, "\t")); err != nil {
		fmt.Fprintf(os.Stderr, "IPC write failed: %v\n", err)
		return 1
	}
	line, err := bufio.NewReader(connection).ReadString('\n')
	if err != nil {
		fmt.Fprintf(os.Stderr, "IPC read failed: %v\n", err)
		return 1
	}
	fmt.Print(line)
	return 0
}
