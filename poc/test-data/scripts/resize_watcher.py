#!/usr/bin/env python3
import fcntl
import os
import signal
import struct
import sys
import termios


def size() -> tuple[int, int]:
    last_error: OSError | None = None
    for descriptor in (sys.stdout.fileno(), sys.stderr.fileno(), sys.stdin.fileno()):
        try:
            packed = fcntl.ioctl(
                descriptor,
                termios.TIOCGWINSZ,
                struct.pack("HHHH", 0, 0, 0, 0),
            )
            rows, columns, _, _ = struct.unpack("HHHH", packed)
            if rows > 0 and columns > 0:
                return rows, columns
        except OSError as error:
            last_error = error
    raise last_error or OSError("terminal size unavailable")


def on_resize(_signum, _frame) -> None:
    rows, columns = size()
    os.write(
        sys.stdout.fileno(),
        f"QWE_WINCH {rows} {columns}\n".encode(),
    )


signal.signal(signal.SIGWINCH, on_resize)
rows, columns = size()
print(f"QWE_RESIZE_READY {rows} {columns}", flush=True)

for line in sys.stdin:
    if line.strip() == "exit":
        break
    rows, columns = size()
    print(f"QWE_SIZE {rows} {columns}", flush=True)
