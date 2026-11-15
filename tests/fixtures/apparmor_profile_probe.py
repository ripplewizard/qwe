#!/usr/bin/env python3

import argparse
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


TARGET_PROFILE = Path("/etc/apparmor.d/qwe-bwrap")
RESTRICT_SYSCTL = Path("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")


def run(arguments, *, check=True, timeout=90, env=None):
    completed = subprocess.run(
        [str(value) for value in arguments],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        env=env,
    )
    if check and completed.returncode != 0:
        raise RuntimeError(
            f"command failed with {completed.returncode}: {arguments}\n"
            f"stdout: {completed.stdout}\nstderr: {completed.stderr}"
        )
    return completed


def sudo(*arguments, **kwargs):
    return run(["sudo", *arguments], **kwargs)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", required=True)
    parser.add_argument("--installer", required=True)
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    parser.add_argument("--stage10-fixture", required=True)
    args = parser.parse_args()

    profile = Path(args.profile).resolve()
    installer = Path(args.installer).resolve()
    qwed = Path(args.qwed).resolve()
    socket_path = Path(args.socket).resolve()
    stage10_fixture = Path(args.stage10_fixture).resolve()
    root = socket_path.parent
    root.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="qwe-apparmor-") as temporary:
        backup = Path(temporary) / "qwe-bwrap"
        had_previous = TARGET_PROFILE.exists()
        if had_previous:
            sudo("cp", TARGET_PROFILE, backup)
        try:
            sudo(installer)
            assert RESTRICT_SYSCTL.read_text().strip() == "1"
            status = sudo("aa-status", timeout=30).stdout
            for name in (
                "qwe-bwrap-usr",
                "qwe-bwrap-usr-local",
                "qwe-bwrap-opt-local",
            ):
                assert name in status, status

            environment = os.environ.copy()
            environment["PYTHONPATH"] = str(stage10_fixture.parent)
            stage10 = run(
                [
                    "python3",
                    stage10_fixture,
                    "--qwed",
                    qwed,
                    "--socket",
                    socket_path,
                ],
                timeout=90,
                env=environment,
            )
            stage10_result = json.loads(stage10.stdout)
            assert stage10_result["status"] == "pass", stage10_result
            print(
                json.dumps(
                    {
                        "case": "SANDBOX-013",
                        "status": "pass",
                        "global_userns_restriction": "enabled",
                        "profile_loaded": "pass",
                        "stage10": stage10_result,
                    },
                    separators=(",", ":"),
                )
            )
        finally:
            for suffix in ("", "-wal", "-shm"):
                path = Path(f"{socket_path}{suffix}")
                if path.exists():
                    path.unlink()
            if had_previous:
                sudo("install", "-D", "-m", "0644", backup, TARGET_PROFILE)
                sudo("apparmor_parser", "-r", TARGET_PROFILE)
            else:
                sudo("apparmor_parser", "-R", str(profile), check=False)
                sudo("rm", "-f", TARGET_PROFILE, check=False)
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
