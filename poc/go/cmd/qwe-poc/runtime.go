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
	PID        int    `json:"pid,omitempty"`
	PGID       int    `json:"pgid,omitempty"`
	StartToken string `json:"start_token,omitempty"`
	State      string `json:"state,omitempty"`
	Created    *bool  `json:"created,omitempty"`
	Error      string `json:"error,omitempty"`
	Message    string `json:"message,omitempty"`
}

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
	`); err != nil {
		database.Close()
		return nil, err
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
	go func() {
		_ = command.Wait()
	}()
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
		return nil, false, err
	}
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

func handleRuntimeRequest(database *sql.DB, line string) runtimeResponse {
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
			var response runtimeResponse
			if readErr != nil {
				response = errorJSON("unknown", "invalid_request", readErr.Error())
			} else {
				response = handleRuntimeRequest(database, line)
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
	if len(arguments) < minimum {
		fmt.Fprintln(os.Stderr, "invalid runtime client arguments")
		return 2
	}
	if operation != "START" && len(arguments) != 2 {
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
