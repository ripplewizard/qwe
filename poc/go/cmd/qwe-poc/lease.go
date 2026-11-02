package main

import (
	"os"
	"strconv"
	"sync"
	"time"
)

type leaseRecord struct {
	owner    string
	deadline time.Time
}

type leaseSuccessResponse struct {
	OK          bool    `json:"ok"`
	RequestID   string  `json:"request_id"`
	SessionID   string  `json:"session_id"`
	State       string  `json:"state"`
	Owner       *string `json:"owner"`
	RemainingMS int64   `json:"remaining_ms"`
	OutputBytes int64   `json:"output_bytes"`
}

type leaseManager struct {
	mutex       sync.Mutex
	idleTimeout time.Duration
	sessions    map[string]leaseRecord
	outputBytes map[string]int64
}

func newLeaseManager() *leaseManager {
	timeout := 600 * time.Second
	if value := os.Getenv("QWE_LEASE_MS"); value != "" {
		if milliseconds, err := strconv.ParseInt(value, 10, 64); err == nil &&
			milliseconds > 0 {
			timeout = time.Duration(milliseconds) * time.Millisecond
		}
	}
	return &leaseManager{
		idleTimeout: timeout,
		sessions:    make(map[string]leaseRecord),
		outputBytes: make(map[string]int64),
	}
}

func (manager *leaseManager) normalized(record leaseRecord, now time.Time) (
	leaseRecord,
	string,
) {
	if record.owner == "" {
		return leaseRecord{}, "idle"
	}
	if now.Before(record.deadline) {
		return record, "owned"
	}
	return record, "expired"
}

func (manager *leaseManager) leaseJSON(
	requestID, sessionID, state, owner string,
	remaining time.Duration,
) leaseSuccessResponse {
	var ownerValue *string
	if owner != "" {
		ownerValue = &owner
	}
	remainingMS := remaining.Milliseconds()
	if remainingMS < 0 {
		remainingMS = 0
	}
	outputBytes := manager.outputBytes[sessionID]
	return leaseSuccessResponse{
		OK:          true,
		RequestID:   requestID,
		SessionID:   sessionID,
		State:       state,
		Owner:       ownerValue,
		RemainingMS: remainingMS,
		OutputBytes: outputBytes,
	}
}

func (manager *leaseManager) claim(
	requestID, sessionID, clientID string,
) any {
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	now := time.Now()
	record, state := manager.normalized(manager.sessions[sessionID], now)
	if state == "owned" {
		return errorJSON(requestID, "already_owned",
			"session is already owned by "+record.owner)
	}
	record = leaseRecord{owner: clientID, deadline: now.Add(manager.idleTimeout)}
	manager.sessions[sessionID] = record
	manager.outputBytes[sessionID] = 0
	return manager.leaseJSON(requestID, sessionID, "owned", clientID,
		time.Until(record.deadline))
}

func (manager *leaseManager) touch(
	requestID, sessionID, clientID string,
) any {
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	now := time.Now()
	record, state := manager.normalized(manager.sessions[sessionID], now)
	if state != "owned" || record.owner != clientID {
		return errorJSON(requestID, "not_owner",
			"client does not own session "+sessionID)
	}
	record.deadline = now.Add(manager.idleTimeout)
	manager.sessions[sessionID] = record
	return manager.leaseJSON(requestID, sessionID, "owned", clientID,
		time.Until(record.deadline))
}

func (manager *leaseManager) release(
	requestID, sessionID, clientID string,
) any {
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	now := time.Now()
	record, state := manager.normalized(manager.sessions[sessionID], now)
	if state != "owned" || record.owner != clientID {
		return errorJSON(requestID, "not_owner",
			"client does not own session "+sessionID)
	}
	delete(manager.sessions, sessionID)
	return manager.leaseJSON(requestID, sessionID, "idle", "", 0)
}

func (manager *leaseManager) status(
	requestID, sessionID string,
) any {
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	now := time.Now()
	record, state := manager.normalized(manager.sessions[sessionID], now)
	if state == "idle" {
		return manager.leaseJSON(requestID, sessionID, state, "", 0)
	}
	if state == "expired" {
		return manager.leaseJSON(requestID, sessionID, state, "", 0)
	}
	manager.sessions[sessionID] = record
	return manager.leaseJSON(requestID, sessionID, state, record.owner,
		record.deadline.Sub(now))
}

func (manager *leaseManager) takeover(
	requestID, sessionID, clientID string,
) any {
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	now := time.Now()
	record, state := manager.normalized(manager.sessions[sessionID], now)
	if state != "expired" {
		return errorJSON(requestID, "not_expired",
			"session lease has not expired")
	}
	record = leaseRecord{owner: clientID, deadline: now.Add(manager.idleTimeout)}
	manager.sessions[sessionID] = record
	manager.outputBytes[sessionID] = 0
	return manager.leaseJSON(requestID, sessionID, "owned", clientID,
		time.Until(record.deadline))
}

func (manager *leaseManager) recordOutput(sessionID string, bytes int64) {
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	manager.outputBytes[sessionID] += bytes
}

func (manager *leaseManager) isOwner(sessionID, clientID string) bool {
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	record, state := manager.normalized(manager.sessions[sessionID], time.Now())
	return state == "owned" && record.owner == clientID
}

func (manager *leaseManager) authorizeInput(sessionID, clientID string) bool {
	manager.mutex.Lock()
	defer manager.mutex.Unlock()
	now := time.Now()
	record, state := manager.normalized(manager.sessions[sessionID], now)
	if state != "owned" || record.owner != clientID {
		return false
	}
	record.deadline = now.Add(manager.idleTimeout)
	manager.sessions[sessionID] = record
	return true
}
