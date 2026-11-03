# Shared Test Data

Shared inputs keep Rust, Go, and C++ comparable. Add fixtures only when the
test specification defines their expected behavior.

- `scripts/`: reusable process, signal, and output workloads
- `shell/`: shell startup and job-control fixtures
- `ansi/`: VT control-sequence fixtures
- `unicode/`: CJK, emoji, combining, and wide-character fixtures
- `processes/`: process-tree definitions
- `workloads/`: deterministic CPU, memory, and output loads

Scripts must be non-interactive by default, accept bounded durations, print
machine-readable identifiers, and clean up children on failure.

`daemon_crash.sh` intentionally sends SIGKILL to a live daemon PID. It is a
failure-injection operation against a real process, not a simulated recovery
result.

`resize_watcher.py` is a real terminal workload used by T07. It reads the
kernel PTY size and handles SIGWINCH without relying on shell trap behavior.

`vt_fixture.py` emits deterministic ANSI/VT and Unicode sequences. Native
terminal emulators parse its PTY output for T05/T06.

`bounded_output.py` emits 100 MiB to validate bounded attach and backpressure.

`continuous_output.py` provides sustained PTY output during concurrency tests.

`attach_benchmark.py` measures concurrent Attach P50/P95/P99 with zero-failure
and latency gates.

