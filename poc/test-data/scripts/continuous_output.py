#!/usr/bin/env python3
import os
import signal
import sys


stopped = False


def stop(_signum, _frame):
    global stopped
    stopped = True


signal.signal(signal.SIGINT, stop)
signal.signal(signal.SIGTERM, stop)

chunk = b"Q" * (64 * 1024)
while not stopped:
    remaining = memoryview(chunk)
    while remaining and not stopped:
        written = os.write(sys.stdout.fileno(), remaining)
        if written == 0:
            raise RuntimeError("PTY write returned zero bytes")
        remaining = remaining[written:]

os.write(sys.stdout.fileno(), b"\nQWE_CONTINUOUS_END\n")
