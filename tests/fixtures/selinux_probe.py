#!/usr/bin/env python3

import argparse
import json
import os
import subprocess
from pathlib import Path


def selinux_mode():
    enforce = Path("/sys/fs/selinux/enforce")
    if enforce.exists():
        return "Enforcing" if enforce.read_text().strip() == "1" else "Permissive"
    if Path("/sys/fs/selinux").exists():
        return "Unknown"
    return "Disabled"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    parser.add_argument("--stage10-fixture", required=True)
    args = parser.parse_args()

    assert selinux_mode() == "Enforcing", selinux_mode()
    stage10_fixture = Path(args.stage10_fixture).resolve()
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(stage10_fixture.parent)
    completed = subprocess.run(
        [
            "python3",
            stage10_fixture,
            "--qwed",
            Path(args.qwed).resolve(),
            "--socket",
            Path(args.socket).resolve(),
        ],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=90,
        env=environment,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"Stage 10 failed under SELinux enforcing\n"
            f"stdout: {completed.stdout}\nstderr: {completed.stderr}"
        )
    stage10 = json.loads(completed.stdout)
    assert stage10["status"] == "pass", stage10
    assert selinux_mode() == "Enforcing", selinux_mode()
    print(
        json.dumps(
            {
                "case": "SANDBOX-016",
                "status": "pass",
                "selinux": "Enforcing",
                "stage10": stage10,
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
