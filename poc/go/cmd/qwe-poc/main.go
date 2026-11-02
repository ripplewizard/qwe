package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"time"

	"github.com/creack/pty"
	"github.com/hinshun/vt10x"
)

type ptyRun struct {
	output   []byte
	exitCode int
}

type readerEvent struct {
	data []byte
	done bool
	err  error
}

func startReader(terminal *os.File) <-chan readerEvent {
	events := make(chan readerEvent, 16)
	go func() {
		defer close(events)
		buffer := make([]byte, 4096)
		for {
			count, err := terminal.Read(buffer)
			if count > 0 {
				data := append([]byte(nil), buffer[:count]...)
				events <- readerEvent{data: data}
			}
			if err == nil {
				continue
			}
			if errors.Is(err, syscall.EIO) || errors.Is(err, io.EOF) {
				events <- readerEvent{done: true}
			} else {
				events <- readerEvent{err: err}
			}
			return
		}
	}()
	return events
}

func readUntil(events <-chan readerEvent, output *[]byte, marker string) (bool, error) {
	start := len(*output)
	deadline := time.NewTimer(10 * time.Second)
	defer deadline.Stop()
	for {
		if bytes.Contains((*output)[start:], []byte(marker)) {
			return true, nil
		}
		select {
		case event, ok := <-events:
			if !ok {
				return false, nil
			}
			if len(event.data) > 0 {
				*output = append(*output, event.data...)
			}
			if event.err != nil {
				return false, event.err
			}
			if event.done {
				return false, nil
			}
		case <-deadline.C:
			return false, fmt.Errorf("PTY read timed out waiting for %q", marker)
		}
	}
}

func readUntilAll(
	events <-chan readerEvent,
	output *[]byte,
	markers []string,
) (bool, error) {
	start := len(*output)
	deadline := time.NewTimer(10 * time.Second)
	defer deadline.Stop()
	for {
		observed := true
		for _, marker := range markers {
			if !bytes.Contains((*output)[start:], []byte(marker)) {
				observed = false
				break
			}
		}
		if observed {
			return true, nil
		}
		select {
		case event, ok := <-events:
			if !ok {
				return false, nil
			}
			if len(event.data) > 0 {
				*output = append(*output, event.data...)
			}
			if event.err != nil {
				return false, event.err
			}
			if event.done {
				return false, nil
			}
		case <-deadline.C:
			return false, fmt.Errorf("PTY read timed out waiting for %v", markers)
		}
	}
}

func readToDone(events <-chan readerEvent, output *[]byte) error {
	deadline := time.NewTimer(10 * time.Second)
	defer deadline.Stop()
	for {
		select {
		case event, ok := <-events:
			if !ok {
				return nil
			}
			if len(event.data) > 0 {
				*output = append(*output, event.data...)
			}
			if event.err != nil {
				return event.err
			}
			if event.done {
				return nil
			}
		case <-deadline.C:
			return fmt.Errorf("PTY read timed out waiting for completion")
		}
	}
}

func runPTYSession(shell, input, logPath string) (ptyRun, error) {
	command := exec.Command(shell)
	command.Env = append(
		os.Environ(),
		"TERM=xterm-256color",
		"NVIM_TEST_MARKER=QWE_NVIM_OK",
	)

	terminal, err := pty.StartWithSize(command, &pty.Winsize{Rows: 24, Cols: 80})
	if err != nil {
		return ptyRun{}, fmt.Errorf("PTY create failed: %w", err)
	}
	defer terminal.Close()

	_, writeErr := terminal.WriteString(input)
	output, readErr := io.ReadAll(terminal)
	waitErr := command.Wait()

	if logPath != "" {
		if logErr := os.WriteFile(logPath, output, 0o644); logErr != nil {
			return ptyRun{}, fmt.Errorf("write PTY log failed: %w", logErr)
		}
	}

	if readErr != nil && !errors.Is(readErr, syscall.EIO) {
		return ptyRun{}, fmt.Errorf("PTY read failed: %w", readErr)
	}
	if writeErr != nil {
		return ptyRun{}, fmt.Errorf("PTY write failed: %w", writeErr)
	}

	exitCode := 0
	if waitErr != nil {
		var exitError *exec.ExitError
		if errors.As(waitErr, &exitError) {
			exitCode = exitError.ExitCode()
		} else {
			return ptyRun{}, fmt.Errorf("PTY child wait failed: %w", waitErr)
		}
	}

	return ptyRun{output: output, exitCode: exitCode}, nil
}

func runPTYSmoke(logPath string) int {
	// Warm up Go's lazily-created runtime polling descriptors before measuring leaks.
	if _, err := runPTYSession("sh", "exit 0\r", ""); err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	before, err := openFDCount()
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	combined := []byte{}
	statuses := map[string]bool{}
	for _, shell := range []string{"bash", "zsh"} {
		run, runErr := runPTYSession(
			shell,
			"printf 'QWE_PTY_READY\\n'; printf 'QWE_PTY_SIZE '; stty size; printf 'QWE_PTY_END\\n'; exit 0\r",
			"",
		)
		if runErr != nil {
			fmt.Fprintln(os.Stderr, runErr)
			return 1
		}
		combined = append(combined, run.output...)
		text := string(run.output)
		statuses[shell] = run.exitCode == 0 &&
			strings.Contains(text, "QWE_PTY_READY") &&
			strings.Contains(text, "QWE_PTY_SIZE 24 80") &&
			strings.Contains(text, "QWE_PTY_END")
	}
	if logPath != "" {
		if err := os.WriteFile(logPath, combined, 0o644); err != nil {
			fmt.Fprintln(os.Stderr, err)
			return 1
		}
	}
	time.Sleep(20 * time.Millisecond)
	after, err := openFDCount()
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	text := string(combined)
	shellCheck := statuses["bash"] && statuses["zsh"]
	sizeCheck := strings.Contains(text, "QWE_PTY_SIZE 24 80")
	fdNoLeak := after <= before
	processReaped := statuses["bash"] && statuses["zsh"]
	passed := shellCheck && sizeCheck && fdNoLeak && processReaped

	fmt.Printf(
		`{"contract":"qwe-poc/0.1","feature":"pty-smoke","implementation":"go","exit_code":0,"bytes":%d,"checks":{"shell":%t,"bash":%t,"zsh":%t,"window_size":%t,"fd_no_leak":%t,"process_reaped":%t}}`+"\n",
		len(combined),
		shellCheck,
		statuses["bash"],
		statuses["zsh"],
		sizeCheck,
		fdNoLeak,
		processReaped,
	)
	if !passed {
		return 1
	}
	return 0
}

func runEditorSmoke(logPath string) int {
	input := "printf 'QWE_EDITOR_SIZE '; stty size; " +
		"nvim --clean -u NONE -c 'call setline(1, $NVIM_TEST_MARKER)' " +
		"-c 'redraw' -c 'sleep 200m' -c 'qa!'; exit $?\r"
	run, err := runPTYSession("sh", input, logPath)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}

	text := string(run.output)
	screenCheck := strings.Contains(text, "QWE_NVIM_OK")
	sizeCheck := strings.Contains(text, "QWE_EDITOR_SIZE 24 80")
	passed := run.exitCode == 0 && screenCheck && sizeCheck

	fmt.Printf(
		`{"contract":"qwe-poc/0.1","feature":"editor-smoke","implementation":"go","exit_code":%d,"bytes":%d,"checks":{"process":%t,"screen_text":%t,"window_size":%t}}`+"\n",
		run.exitCode,
		len(run.output),
		run.exitCode == 0,
		screenCheck,
		sizeCheck,
	)
	if !passed {
		return 1
	}
	return 0
}

func runLongOutputSmoke(logPath, scriptPath string) int {
	const lineCount = 50000
	absoluteScript, err := filepath.Abs(scriptPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "resolve long output script failed: %v\n", err)
		return 1
	}
	quoted := strings.ReplaceAll(absoluteScript, "'", "'\\''")
	started := time.Now()
	run, err := runPTYSession(
		"sh",
		fmt.Sprintf("exec '%s' %d\r", quoted, lineCount),
		logPath,
	)
	elapsed := time.Since(started).Milliseconds()
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	text := string(run.output)
	lines := strings.Count(text, "QWE-LINE-")
	countOK := lines == lineCount
	firstOK := strings.Contains(text, "QWE-LINE-00050000")
	lastOK := strings.Contains(text, "QWE-LINE-00000001")
	passed := run.exitCode == 0 && countOK && firstOK && lastOK
	fmt.Printf(
		`{"contract":"qwe-poc/0.1","feature":"long-output","implementation":"go","exit_code":%d,"bytes":%d,"lines":%d,"elapsed_ms":%d,"checks":{"count":%t,"first":%t,"last":%t}}`+"\n",
		run.exitCode,
		len(run.output),
		lines,
		elapsed,
		countOK,
		firstOK,
		lastOK,
	)
	if !passed {
		return 1
	}
	return 0
}

func runVTSmoke(logPath, fixturePath string) int {
	absoluteFixture, err := filepath.Abs(fixturePath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "resolve VT fixture failed: %v\n", err)
		return 1
	}
	quoted := strings.ReplaceAll(absoluteFixture, "'", "'\\''")
	run, err := runPTYSession(
		"sh",
		fmt.Sprintf("exec python3 '%s'\r", quoted),
		"",
	)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	altRun, err := runPTYSession(
		"sh",
		fmt.Sprintf("exec python3 '%s' alt\r", quoted),
		"",
	)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	combined := append(append([]byte{}, run.output...), altRun.output...)
	if logPath != "" {
		if err := os.WriteFile(logPath, combined, 0o644); err != nil {
			fmt.Fprintln(os.Stderr, err)
			return 1
		}
	}

	terminal := vt10x.New(vt10x.WithSize(80, 24))
	_, _ = terminal.Write(run.output)
	text := terminal.String()
	cell00 := terminal.Cell(0, 0)
	cellR := terminal.Cell(0, 1)
	cellB := terminal.Cell(4, 1)
	cellErase := terminal.Cell(1, 2)
	cursor := terminal.Cursor()
	cjkCell := terminal.Cell(0, 3)
	rows := strings.Split(text, "\n")
	baseText := strings.Contains(text, "QWE_BASE")
	colors := cellR.Char == 'R' && cellR.FG == vt10x.Red &&
		cellB.Char == 'B' &&
		(cellB.FG == vt10x.Blue || cellB.FG == vt10x.LightBlue)
	erase := cell00.Char == 'Q' && cellErase.Char == ' '
	cursorOK := cursor.X == 10 && cursor.Y == 5 && !terminal.CursorVisible()
	unicodeOK := strings.Contains(text, "中文") &&
		strings.Contains(text, "😀")
	wideOK := cjkCell.Char == '中' && len(rows) > 3 &&
		strings.HasPrefix(rows[3], "中文 😀 e")
	titleOK := terminal.Title() == "QWE_VT_TEST"
	scrollOK := len(rows) > 8 &&
		strings.TrimRight(rows[6], " ") == "OUTSIDE" &&
		strings.TrimRight(rows[7], " ") == "SCROLL9" &&
		strings.TrimRight(rows[8], " ") == "SCROLL10"
	altTerminal := vt10x.New(vt10x.WithSize(80, 24))
	_, _ = altTerminal.Write(altRun.output)
	alternateOK := altRun.exitCode == 0 &&
		altTerminal.Mode()&vt10x.ModeAltScreen != 0 &&
		strings.Contains(altTerminal.String(), "ALT_SCREEN")
	t05 := run.exitCode == 0 && baseText && colors && erase && cursorOK &&
		titleOK && scrollOK && alternateOK
	t06 := run.exitCode == 0 && unicodeOK && wideOK
	fmt.Printf(
		`{"contract":"qwe-poc/0.1","feature":"vt-state","implementation":"go","exit_code":%d,"checks":{"base_text":%t,"colors":%t,"erase":%t,"cursor":%t,"title":%t,"scroll_region":%t,"alternate_screen":%t,"unicode":%t,"wide":%t}}`+"\n",
		run.exitCode,
		baseText,
		colors,
		erase,
		cursorOK,
		titleOK,
		scrollOK,
		alternateOK,
		unicodeOK,
		wideOK,
	)
	if !t05 || !t06 {
		fmt.Fprintf(
			os.Stderr,
			"VT diagnostic: row1=%q/%d %q/%d erase=%q cjk=%q row3=%q cursor=%d,%d visible=%t\n",
			cellR.Char,
			cellR.FG,
			cellB.Char,
			cellB.FG,
			cellErase.Char,
			cjkCell.Char,
			func() string {
				if len(rows) > 3 {
					return rows[3]
				}
				return ""
			}(),
			cursor.X,
			cursor.Y,
			terminal.CursorVisible(),
		)
		return 1
	}
	return 0
}

func openFDCount() (int, error) {
	path := "/proc/self/fd"
	if runtime.GOOS == "darwin" {
		path = "/dev/fd"
	}
	entries, err := os.ReadDir(path)
	if err != nil {
		return 0, err
	}
	return len(entries), nil
}

func runResizeSmoke(logPath, watcherPath string) int {
	absoluteWatcher, err := filepath.Abs(watcherPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "resolve resize watcher failed: %v\n", err)
		return 1
	}
	command := exec.Command("sh")
	command.Env = append(
		os.Environ(),
		"TERM=xterm-256color",
		"QWE_WATCHER_PATH="+absoluteWatcher,
	)

	terminal, err := pty.StartWithSize(command, &pty.Winsize{Rows: 24, Cols: 80})
	if err != nil {
		fmt.Fprintf(os.Stderr, "PTY create failed: %v\n", err)
		return 1
	}
	defer terminal.Close()

	events := startReader(terminal)
	if _, err := terminal.WriteString(`exec python3 "$QWE_WATCHER_PATH"` + "\r"); err != nil {
		fmt.Fprintf(os.Stderr, "PTY write failed: %v\n", err)
		return 1
	}
	var output []byte
	ready, err := readUntil(events, &output, "QWE_RESIZE_READY 24 80")
	if err != nil {
		if logPath != "" {
			_ = os.WriteFile(logPath, output, 0o644)
		}
		fmt.Fprintf(os.Stderr, "PTY resize setup failed: %v; output=%q\n", err, output)
		return 1
	}
	if !ready {
		if logPath != "" {
			_ = os.WriteFile(logPath, output, 0o644)
		}
		fmt.Fprintf(os.Stderr, "PTY resize setup ended before ready; output=%q\n", output)
		return 1
	}

	sizes := []struct {
		cols uint16
		rows uint16
	}{
		{120, 40},
		{80, 24},
		{160, 50},
		{40, 15},
	}
	sizeChecks := true
	winchChecks := true
	emulatorChecks := true
	emulator := vt10x.New(vt10x.WithSize(80, 24))
	sizeLabels := make([]string, 0, len(sizes))
	for _, size := range sizes {
		if err := pty.Setsize(terminal, &pty.Winsize{
			Rows: size.rows,
			Cols: size.cols,
		}); err != nil {
			fmt.Fprintf(os.Stderr, "PTY resize failed: %v\n", err)
			return 1
		}
		emulator.Resize(int(size.cols), int(size.rows))
		emulatorCols, emulatorRows := emulator.Size()
		emulatorChecks = emulatorChecks &&
			emulatorCols == int(size.cols) &&
			emulatorRows == int(size.rows)
		if _, err := terminal.WriteString("check\r"); err != nil {
			fmt.Fprintf(os.Stderr, "PTY write failed: %v\n", err)
			return 1
		}
		time.Sleep(10 * time.Millisecond)
		if err := syscall.Kill(-command.Process.Pid, syscall.SIGWINCH); err != nil {
			fmt.Fprintf(os.Stderr, "PTY resize notification failed: %v\n", err)
			return 1
		}
		time.Sleep(10 * time.Millisecond)
		if err := syscall.Kill(-command.Process.Pid, syscall.SIGWINCH); err != nil {
			fmt.Fprintf(os.Stderr, "PTY resize notification failed: %v\n", err)
			return 1
		}

		sizeMarker := fmt.Sprintf("QWE_SIZE %d %d", size.rows, size.cols)
		winchMarker := fmt.Sprintf(
			"QWE_WINCH %d %d",
			size.rows,
			size.cols,
		)
		observed, err := readUntilAll(
			events,
			&output,
			[]string{sizeMarker, winchMarker},
		)
		if err != nil {
			if logPath != "" {
				_ = os.WriteFile(logPath, output, 0o644)
			}
			fmt.Fprintf(
				os.Stderr,
				"PTY resize %dx%d failed: %v; output=%q\n",
				size.rows,
				size.cols,
				err,
				output,
			)
			return 1
		}
		sizeChecks = sizeChecks && observed
		winchChecks = winchChecks && observed
		sizeLabels = append(
			sizeLabels,
			fmt.Sprintf("%dx%d", size.rows, size.cols),
		)
	}

	if _, err := terminal.WriteString("exit\r"); err != nil {
		fmt.Fprintf(os.Stderr, "PTY write failed: %v\n", err)
		return 1
	}
	if err := readToDone(events, &output); err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	waitErr := command.Wait()
	exitCode := 0
	if waitErr != nil {
		var exitError *exec.ExitError
		if errors.As(waitErr, &exitError) {
			exitCode = exitError.ExitCode()
		} else {
			fmt.Fprintf(os.Stderr, "PTY child wait failed: %v\n", waitErr)
			return 1
		}
	}

	if logPath != "" {
		if err := os.WriteFile(logPath, output, 0o644); err != nil {
			fmt.Fprintf(os.Stderr, "write PTY log failed: %v\n", err)
			return 1
		}
	}

	passed := ready && sizeChecks && winchChecks && emulatorChecks && exitCode == 0
	sizesJSON, _ := json.Marshal(sizeLabels)
	fmt.Printf(
		`{"contract":"qwe-poc/0.1","feature":"pty-resize","implementation":"go","exit_code":%d,"bytes":%d,"checks":{"ready":%t,"size":%t,"winch":%t,"emulator":%t},"sizes":%s}`+"\n",
		exitCode,
		len(output),
		ready,
		sizeChecks,
		winchChecks,
		emulatorChecks,
		sizesJSON,
	)
	if !passed {
		return 1
	}
	return 0
}

func main() {
	if len(os.Args) < 2 {
		fmt.Println(
			"usage: qwe-poc <smoke|daemon|task-*|lease-*|session-start>",
		)
		return
	}

	switch os.Args[1] {
	case "probe":
		fmt.Println(`{"contract":"qwe-poc/0.1","implementation":"go"}`)
	case "pty-smoke":
		logPath := ""
		if len(os.Args) > 2 {
			logPath = os.Args[2]
		}
		os.Exit(runPTYSmoke(logPath))
	case "editor-smoke":
		logPath := ""
		if len(os.Args) > 2 {
			logPath = os.Args[2]
		}
		os.Exit(runEditorSmoke(logPath))
	case "long-output-smoke":
		logPath := ""
		if len(os.Args) > 2 {
			logPath = os.Args[2]
		}
		scriptPath := "poc/test-data/scripts/long_output.sh"
		if len(os.Args) > 3 {
			scriptPath = os.Args[3]
		}
		os.Exit(runLongOutputSmoke(logPath, scriptPath))
	case "vt-smoke":
		logPath := ""
		if len(os.Args) > 2 {
			logPath = os.Args[2]
		}
		fixturePath := "poc/test-data/scripts/vt_fixture.py"
		if len(os.Args) > 3 {
			fixturePath = os.Args[3]
		}
		os.Exit(runVTSmoke(logPath, fixturePath))
	case "resize-smoke":
		logPath := ""
		if len(os.Args) > 2 {
			logPath = os.Args[2]
		}
		watcherPath := "poc/test-data/scripts/resize_watcher.py"
		if len(os.Args) > 3 {
			watcherPath = os.Args[3]
		}
		os.Exit(runResizeSmoke(logPath, watcherPath))
	case "daemon":
		if len(os.Args) != 4 {
			fmt.Fprintln(os.Stderr, "usage: qwe-poc daemon <socket-path> <sqlite-path>")
			os.Exit(2)
		}
		if err := runRuntimeDaemon(os.Args[2], os.Args[3]); err != nil {
			fmt.Fprintf(os.Stderr, "daemon failed: %v\n", err)
			os.Exit(1)
		}
	case "task-start":
		os.Exit(runRuntimeClient("START"))
	case "task-start-timeout":
		os.Exit(runRuntimeClient("TIMEOUT_START"))
	case "task-start-restart":
		os.Exit(runRuntimeClient("RESTART_START"))
	case "task-restart-status":
		os.Exit(runRuntimeClient("RESTART_STATUS"))
	case "task-status":
		os.Exit(runRuntimeClient("STATUS"))
	case "task-stop":
		os.Exit(runRuntimeClient("STOP"))
	case "task-history":
		os.Exit(runRuntimeClient("TASK_HISTORY"))
	case "task-signal":
		os.Exit(runRuntimeClient("TASK_SIGNAL"))
	case "task-resource":
		os.Exit(runRuntimeClient("TASK_RESOURCE"))
	case "task-resource-sample":
		os.Exit(runRuntimeClient("TASK_RESOURCE_SAMPLE"))
	case "lease-claim":
		os.Exit(runRuntimeClient("LEASE_CLAIM"))
	case "lease-touch":
		os.Exit(runRuntimeClient("LEASE_TOUCH"))
	case "lease-release":
		os.Exit(runRuntimeClient("LEASE_RELEASE"))
	case "lease-status":
		os.Exit(runRuntimeClient("LEASE_STATUS"))
	case "lease-takeover":
		os.Exit(runRuntimeClient("LEASE_TAKEOVER"))
	case "session-start":
		os.Exit(runRuntimeClient("SESSION_START"))
	case "session-attach":
		os.Exit(runRuntimeClient("SESSION_ATTACH"))
	case "session-detach":
		os.Exit(runRuntimeClient("SESSION_DETACH"))
	case "session-input":
		os.Exit(runRuntimeClient("SESSION_INPUT"))
	case "session-status":
		os.Exit(runRuntimeClient("SESSION_STATUS"))
	case "dashboard":
		if len(os.Args) != 3 {
			fmt.Fprintln(os.Stderr, "usage: qwe-poc dashboard <sqlite-path>")
			os.Exit(2)
		}
		os.Exit(runDashboard(os.Args[2]))
	case "help":
		fmt.Println(
			"usage: qwe-poc <smoke|daemon|task-*|lease-*|session-start>",
		)
	default:
		fmt.Fprintf(os.Stderr, "unsupported command: %s\n", os.Args[1])
		os.Exit(2)
	}
}
