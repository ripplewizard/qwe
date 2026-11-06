#!/usr/bin/env python3

import argparse
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def run(arguments, *, check=True, timeout=30):
    completed = subprocess.run(
        [str(value) for value in arguments],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
    )
    if check and completed.returncode != 0:
        raise RuntimeError(
            f"command failed with {completed.returncode}: {arguments}\n"
            f"stdout: {completed.stdout}\nstderr: {completed.stderr}"
        )
    return completed


def wait_until(predicate, description, timeout=20):
    deadline = time.monotonic() + timeout
    failure = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except Exception as error:
            failure = error
        time.sleep(0.05)
    raise RuntimeError(f"timed out waiting for {description}: {failure}")


def process_alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def wait_process_gone(pid, timeout=10):
    wait_until(lambda: not process_alive(pid), f"process {pid} to exit", timeout)


def qwe_call(qwe, socket_path, *arguments, timeout=20):
    completed = run(
        [qwe, "--socket", socket_path, *arguments],
        timeout=timeout,
    )
    output = completed.stdout.strip()
    return json.loads(output) if output else {}


class UserService:
    def __init__(self, unit_path, dropin_dir, systemctl):
        self.unit_path = unit_path
        self.dropin_dir = dropin_dir
        self.systemctl = systemctl
        self.unit = unit_path.name

    def command(self, *arguments, check=True):
        return run([self.systemctl, "--user", *arguments], check=check)

    def install(self, installed_unit, qwed, socket_path, store_path):
        self.unit_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(installed_unit, self.unit_path)
        self.unit_path.chmod(0o644)
        self.dropin_dir.mkdir(parents=True, exist_ok=True)
        (self.dropin_dir / "validation.conf").write_text(
            "\n".join(
                [
                    "[Service]",
                    "ExecStart=",
                    (
                        f'ExecStart="{qwed}" --socket "{socket_path}" '
                        f'--store "{store_path}"'
                    ),
                    "RuntimeDirectory=",
                    "StateDirectory=",
                    "PrivateTmp=false",
                    "ProtectSystem=false",
                    "ProtectHome=false",
                    "Restart=on-failure",
                    "RestartSec=1",
                    "KillMode=process",
                    "",
                ]
            ),
            encoding="utf-8",
        )
        (self.dropin_dir / "validation.conf").chmod(0o644)
        self.command("daemon-reload")
        self.command("start", self.unit)

    def main_pid(self):
        output = self.command(
            "show",
            self.unit,
            "-p",
            "MainPID",
            "--value",
        ).stdout.strip()
        return int(output) if output.isdigit() else 0

    def stop(self):
        self.command("stop", self.unit, check=False)

    def uninstall(self):
        self.stop()
        self.unit_path.unlink(missing_ok=True)
        if self.dropin_dir.exists():
            shutil.rmtree(self.dropin_dir)
        self.command("daemon-reload", check=False)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--systemctl", default="/usr/bin/systemctl")
    args = parser.parse_args()

    if sys.platform != "linux":
        raise SystemExit("release service probe requires Linux")

    package = args.package.resolve()
    manifest = json.loads((package / "release.json").read_text(encoding="utf-8"))
    if manifest["target"] != "linux-x86_64":
        raise SystemExit(f"unexpected release target: {manifest['target']}")

    qwed_process = 0
    worker_pid = None
    task_pid = None
    service = None
    with tempfile.TemporaryDirectory(
        prefix="qwe-release-service-", dir="/tmp"
    ) as root:
        root = Path(root)
        prefix = root / "home"
        socket_dir = root / "run"
        state_dir = root / "state"
        socket_dir.mkdir(mode=0o700)
        state_dir.mkdir(mode=0o700)
        socket_path = socket_dir / "qwe.sock"
        store_path = state_dir / "qwe.sqlite"

        installed = json.loads(
            run(
                [
                    "python3",
                    package / "install.py",
                    "--prefix",
                    prefix,
                ]
            ).stdout
        )
        assert installed["distribution_profile"] == "unsigned"
        bin_dir = prefix / ".local/bin"
        qwe = bin_dir / "qwe"
        installed_unit = prefix / ".config/systemd/user/qwe.service"
        assert installed_unit.is_file()

        unit_name = f"qwe-stage5-release-{os.getpid()}.service"
        unit_path = Path.home() / ".config/systemd/user" / unit_name
        dropin_dir = unit_path.with_name(f"{unit_name}.d")
        service = UserService(unit_path, dropin_dir, args.systemctl)
        try:
            service.install(installed_unit, bin_dir / "qwed", socket_path, store_path)
            qwed_process = wait_until(
                service.main_pid,
                "initial qwed service PID",
            )
            wait_until(
                lambda: qwe_call(qwe, socket_path, "ping").get("daemon") == "ok",
                "installed qwe service readiness",
            )
            created = qwe_call(
                qwe,
                socket_path,
                "session",
                "create",
                "release-service",
                "sh",
                "--",
                "-c",
                "sleep 60",
            )
            worker_pid = created["worker_pid"]
            task_pid = created["pid"]
            assert created["running"] is True
            assert store_path.is_file() and store_path.stat().st_size > 0

            reinstalled = json.loads(
                run(
                    [
                        "python3",
                        package / "install.py",
                        "--prefix",
                        prefix,
                    ]
                ).stdout
            )
            assert reinstalled["distribution_profile"] == "unsigned"

            crashed_qwed = qwed_process
            os.kill(qwed_process, signal.SIGKILL)
            wait_process_gone(qwed_process)
            assert process_alive(worker_pid)

            qwed_process = wait_until(
                lambda: (
                    pid
                    if (pid := service.main_pid()) and pid != crashed_qwed
                    else None
                ),
                "systemd qwed restart",
                timeout=25,
            )
            wait_until(
                lambda: qwe_call(qwe, socket_path, "ping").get("daemon") == "ok",
                "restarted qwe service readiness",
            )
            status = qwe_call(
                qwe,
                socket_path,
                "session",
                "status",
                "release-service",
            )
            assert status["running"] is True
            assert status["worker_pid"] == worker_pid
            assert status["pid"] == task_pid

            service.stop()
            wait_process_gone(qwed_process)
            qwed_process = 0
            wait_process_gone(worker_pid)
            wait_process_gone(task_pid)
            worker_pid = None
            task_pid = None

            with sqlite3.connect(store_path) as connection:
                session_state = connection.execute(
                    "SELECT state FROM sessions WHERE id = 'release-service'"
                ).fetchone()[0]
                task_state = connection.execute(
                    "SELECT state FROM tasks WHERE session_id = 'release-service'"
                ).fetchone()[0]
            assert session_state == "closed"
            assert task_state == "stopped"
        finally:
            if service is not None:
                service.uninstall()
            for pid in (qwed_process, worker_pid, task_pid):
                if pid:
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    print(
        json.dumps(
            {
                "case": "PACK-005",
                "installed_systemd_unit": "pass",
                "binary_reinstall_preserved_store": "pass",
                "systemd_crash_restart": "pass",
                "worker_and_task_adopted": "pass",
                "graceful_stop_cleanup": "pass",
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
