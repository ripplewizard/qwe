#!/usr/bin/env python3
import os
import sys

chunk = b"X" * (1024 * 1024)
for _ in range(100):
    remaining = memoryview(chunk)
    while remaining:
        written = os.write(sys.stdout.fileno(), remaining)
        if written == 0:
            raise RuntimeError("PTY write returned zero bytes")
        remaining = remaining[written:]
os.write(sys.stdout.fileno(), b"\nQWE_BOUNDED_END\n")
