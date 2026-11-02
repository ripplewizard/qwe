package main

import (
	"encoding/hex"
	"os"
	"os/exec"
	"sync"

	"github.com/creack/pty"
)

type sessionRecord struct {
	terminal *os.File
	command  *exec.Cmd
	logFile  *os.File
	pid      int
	running  bool
	attached bool
	output   []byte
}

type sessionManager struct {
	mutex    sync.Mutex
	sessions map[string]*sessionRecord
	leases   *leaseManager
}

type sessionResponse struct {
	OK          bool   `json:"ok"`
	RequestID   string `json:"request_id"`
	SessionID   string `json:"session_id"`
	PID         int    `json:"pid"`
	Running     bool   `json:"running"`
	Attached    bool   `json:"attached"`
	OutputBytes int64  `json:"output_bytes"`
	SnapshotHex string `json:"snapshot_hex"`
	Error       string `json:"error,omitempty"`
	Message     string `json:"message,omitempty"`
}

func newSessionManager(leases *leaseManager) *sessionManager {
	return &sessionManager{
		sessions: make(map[string]*sessionRecord),
		leases:   leases,
	}
}

func (manager *sessionManager) response(
	requestID, sessionID string,
	record *sessionRecord,
) sessionResponse {
	return sessionResponse{
		OK:          true,
		RequestID:   requestID,
		SessionID:   sessionID,
		PID:         record.pid,
		Running:     record.running,
		Attached:    record.attached,
		OutputBytes: int64(len(record.output)),
		SnapshotHex: hex.EncodeToString(record.output),
	}
}

func sessionError(requestID, code, message string) sessionResponse {
	return sessionResponse{
		OK:        false,
		RequestID: requestID,
		Error:     code,
		Message:   message,
	}
}

func (manager *sessionManager) start(
	requestID, sessionID, logPath, program string,
	arguments []string,
) sessionResponse {
	record := &sessionRecord{}
	manager.mutex.Lock()
	if _, exists := manager.sessions[sessionID]; exists {
		manager.mutex.Unlock()
		return sessionError(requestID, "already_exists",
			"session already exists: "+sessionID)
	}
	manager.sessions[sessionID] = record
	manager.mutex.Unlock()
	abortStart := func() {
		manager.mutex.Lock()
		if manager.sessions[sessionID] == record {
			delete(manager.sessions, sessionID)
		}
		manager.mutex.Unlock()
	}

	logFile, err := os.OpenFile(
		logPath,
		os.O_CREATE|os.O_TRUNC|os.O_WRONLY,
		0o644,
	)
	if err != nil {
		abortStart()
		return sessionError(requestID, "internal", err.Error())
	}
	command := exec.Command(program, arguments...)
	terminal, err := pty.StartWithSize(
		command,
		&pty.Winsize{Rows: 24, Cols: 80},
	)
	if err != nil {
		logFile.Close()
		abortStart()
		return sessionError(requestID, "internal", err.Error())
	}
	manager.mutex.Lock()
	record.terminal = terminal
	record.command = command
	record.logFile = logFile
	record.pid = command.Process.Pid
	record.running = true
	record.output = []byte{}

	go func() {
		defer terminal.Close()
		defer logFile.Close()
		buffer := make([]byte, 4096)
		for {
			count, readErr := terminal.Read(buffer)
			if count > 0 {
				chunk := buffer[:count]
				_, _ = logFile.Write(chunk)
				manager.mutex.Lock()
				record.output = append(record.output, chunk...)
				manager.mutex.Unlock()
				manager.leases.recordOutput(sessionID, int64(count))
			}
			if readErr != nil {
				break
			}
		}
		_ = command.Wait()
		manager.mutex.Lock()
		record.running = false
		record.attached = false
		manager.mutex.Unlock()
	}()

	response := manager.response(requestID, sessionID, record)
	manager.mutex.Unlock()
	return response
}

func (manager *sessionManager) attach(
	requestID, sessionID, clientID string,
) sessionResponse {
	if !manager.leases.authorizeInput(sessionID, clientID) {
		return sessionError(requestID, "not_owner",
			"client does not own session "+sessionID)
	}
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	record, ok := manager.sessions[sessionID]
	if !ok {
		return sessionError(requestID, "not_found",
			"session not found: "+sessionID)
	}
	record.attached = true
	return manager.response(requestID, sessionID, record)
}

func (manager *sessionManager) detach(
	requestID, sessionID, clientID string,
) sessionResponse {
	if !manager.leases.isOwner(sessionID, clientID) {
		return sessionError(requestID, "not_owner",
			"client does not own session "+sessionID)
	}
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	record, ok := manager.sessions[sessionID]
	if !ok {
		return sessionError(requestID, "not_found",
			"session not found: "+sessionID)
	}
	record.attached = false
	return manager.response(requestID, sessionID, record)
}

func (manager *sessionManager) input(
	requestID, sessionID, clientID, encoded string,
) sessionResponse {
	if !manager.leases.isOwner(sessionID, clientID) {
		return sessionError(requestID, "not_owner",
			"client does not own session "+sessionID)
	}
	data, err := hex.DecodeString(encoded)
	if err != nil {
		return sessionError(requestID, "invalid_request", err.Error())
	}
	manager.mutex.Lock()
	record, ok := manager.sessions[sessionID]
	if !ok {
		manager.mutex.Unlock()
		return sessionError(requestID, "not_found",
			"session not found: "+sessionID)
	}
	if !record.running {
		manager.mutex.Unlock()
		return sessionError(requestID, "not_running",
			"session is not running: "+sessionID)
	}
	if !record.attached {
		manager.mutex.Unlock()
		return sessionError(requestID, "not_attached",
			"session is detached: "+sessionID)
	}
	_, writeErr := record.terminal.Write(data)
	response := manager.response(requestID, sessionID, record)
	manager.mutex.Unlock()
	if writeErr != nil {
		return sessionError(requestID, "internal", writeErr.Error())
	}
	return response
}

func (manager *sessionManager) status(
	requestID, sessionID string,
) sessionResponse {
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	record, ok := manager.sessions[sessionID]
	if !ok {
		return sessionError(requestID, "not_found",
			"session not found: "+sessionID)
	}
	return manager.response(requestID, sessionID, record)
}
