package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"syscall"
	"time"

	"github.com/creack/pty"
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

func runPTYSession(input, logPath string) (ptyRun, error) {
	command := exec.Command("sh")
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
	run, err := runPTYSession(
		"printf 'QWE_PTY_READY\\n'; printf 'QWE_PTY_SIZE '; stty size; printf 'QWE_PTY_END\\n'; exit 0\r",
		logPath,
	)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}

	text := string(run.output)
	shellCheck := strings.Contains(text, "QWE_PTY_READY") &&
		strings.Contains(text, "QWE_PTY_END")
	sizeCheck := strings.Contains(text, "QWE_PTY_SIZE 24 80")
	passed := run.exitCode == 0 && shellCheck && sizeCheck

	fmt.Printf(
		`{"contract":"qwe-poc/0.1","feature":"pty-smoke","implementation":"go","exit_code":%d,"bytes":%d,"checks":{"shell":%t,"window_size":%t}}`+"\n",
		run.exitCode,
		len(run.output),
		shellCheck,
		sizeCheck,
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
	run, err := runPTYSession(input, logPath)
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

func runResizeSmoke(logPath string) int {
	command := exec.Command("sh")
	command.Env = append(
		os.Environ(),
		"TERM=xterm-256color",
		"QWE_READY_MARKER=QWE_RESIZE_READY",
		"QWE_WINCH_MARKER=QWE_WINCH",
		"QWE_SIZE_MARKER=QWE_SIZE",
	)

	terminal, err := pty.StartWithSize(command, &pty.Winsize{Rows: 24, Cols: 80})
	if err != nil {
		fmt.Fprintf(os.Stderr, "PTY create failed: %v\n", err)
		return 1
	}
	defer terminal.Close()

	events := startReader(terminal)
	_, err = terminal.WriteString(
		"trap 'printf \"%s \" \"$QWE_WINCH_MARKER\"; stty size' WINCH; " +
			"printf '%s ' \"$QWE_READY_MARKER\"; stty size; " +
			"while IFS= read -r line; do " +
			"[ \"$line\" = exit ] && break; " +
			"printf '%s ' \"$QWE_SIZE_MARKER\"; stty size; done; exit 0\r",
	)
	if err != nil {
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
	sizeLabels := make([]string, 0, len(sizes))
	for _, size := range sizes {
		start := len(output)
		if err := pty.Setsize(terminal, &pty.Winsize{
			Rows: size.rows,
			Cols: size.cols,
		}); err != nil {
			fmt.Fprintf(os.Stderr, "PTY resize failed: %v\n", err)
			return 1
		}
		if runtime.GOOS == "darwin" {
			if err := syscall.Kill(-command.Process.Pid, syscall.SIGWINCH); err != nil {
				fmt.Fprintf(os.Stderr, "PTY resize notification failed: %v\n", err)
				return 1
			}
		}
		if _, err := terminal.WriteString("check\r"); err != nil {
			fmt.Fprintf(os.Stderr, "PTY write failed: %v\n", err)
			return 1
		}

		sizeMarker := fmt.Sprintf("QWE_SIZE %d %d", size.rows, size.cols)
		observedSize, err := readUntil(events, &output, sizeMarker)
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
		winchMarker := fmt.Sprintf(
			"QWE_WINCH %d %d",
			size.rows,
			size.cols,
		)
		observedWinch := bytes.Contains(output[start:], []byte(winchMarker))
		sizeChecks = sizeChecks && observedSize
		winchChecks = winchChecks && observedWinch
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

	passed := ready && sizeChecks && winchChecks && exitCode == 0
	sizesJSON, _ := json.Marshal(sizeLabels)
	fmt.Printf(
		`{"contract":"qwe-poc/0.1","feature":"pty-resize","implementation":"go","exit_code":%d,"bytes":%d,"checks":{"ready":%t,"size":%t,"winch":%t},"sizes":%s}`+"\n",
		exitCode,
		len(output),
		ready,
		sizeChecks,
		winchChecks,
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
			"usage: qwe-poc <probe|pty-smoke|editor-smoke|resize-smoke> [log-path]",
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
	case "resize-smoke":
		logPath := ""
		if len(os.Args) > 2 {
			logPath = os.Args[2]
		}
		os.Exit(runResizeSmoke(logPath))
	case "help":
		fmt.Println(
			"usage: qwe-poc <probe|pty-smoke|editor-smoke|resize-smoke> [log-path]",
		)
	default:
		fmt.Fprintf(os.Stderr, "unsupported command: %s\n", os.Args[1])
		os.Exit(2)
	}
}
