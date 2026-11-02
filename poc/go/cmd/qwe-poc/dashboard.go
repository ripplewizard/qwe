package main

import (
	"database/sql"
	"fmt"
	"os"
	"strings"
)

func runDashboard(databasePath string) int {
	database, err := sql.Open("sqlite3", databasePath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "dashboard failed: %v\n", err)
		return 1
	}
	defer database.Close()

	rows, err := database.Query(
		"SELECT state, COUNT(*) FROM tasks GROUP BY state ORDER BY state",
	)
	if err != nil {
		fmt.Fprintf(os.Stderr, "dashboard failed: %v\n", err)
		return 1
	}
	defer rows.Close()
	states := make([]string, 0)
	taskCount := 0
	for rows.Next() {
		var state string
		var count int
		if err := rows.Scan(&state, &count); err != nil {
			fmt.Fprintf(os.Stderr, "dashboard failed: %v\n", err)
			return 1
		}
		states = append(states, fmt.Sprintf("%s=%d", state, count))
		taskCount += count
	}
	if err := rows.Err(); err != nil {
		fmt.Fprintf(os.Stderr, "dashboard failed: %v\n", err)
		return 1
	}
	if len(states) == 0 {
		states = append(states, "none")
	}

	latest := "latest: none"
	var id, key, state string
	err = database.QueryRow(
		"SELECT id, idempotency_key, state FROM tasks ORDER BY updated_at DESC LIMIT 1",
	).Scan(&id, &key, &state)
	if err == nil {
		latest = fmt.Sprintf("latest: id=%s key=%s state=%s", id, key, state)
	} else if err != sql.ErrNoRows {
		fmt.Fprintf(os.Stderr, "dashboard failed: %v\n", err)
		return 1
	}

	fmt.Printf(
		"\x1b[?1049h\x1b[H\x1b[2JQWE POC DASHBOARD\r\n"+
			"database: %s\r\n"+
			"tasks=%d %s\r\n"+
			"%s\r\n\r\n"+
			"[q] quit (one-shot POC screen)\x1b[?1049l",
		databasePath,
		taskCount,
		strings.Join(states, " "),
		latest,
	)
	return 0
}
