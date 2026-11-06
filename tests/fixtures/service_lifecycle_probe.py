#!/usr/bin/env python3

import argparse
import json
import os
import plistlib
import re
import signal
import sqlite3
import subprocess
import sys
import time
from pathlib import Path


def run(arguments, *, check=True, timeout=15):
    return subprocess.run(
        [str(value) for value in arguments],
        check=check,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
    )


def wait_until(predicate, description, timeout=15):
    deadline = time.monotonic() + timeout
    last_failure = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except Exception as failure:
            last_failure = failure
        time.sleep(0.05)
    raise RuntimeError(f"timed out waiting for {description}: {last_failure}")


def process_alive(pid):
    if not pid:
        return False
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def wait_process_gone(pid, description, timeout=10):
    wait_until(lambda: not process_alive(pid), description, timeout)


class QweClient:
    def __init__(self, qwe, socket_path):
        self.qwe = qwe
        self.socket_path = socket_path

    def run(self, *arguments):
        completed = run(
            [self.qwe, "--socket", self.socket_path, *arguments],
            timeout=20,
        )
        output = completed.stdout.strip()
        return json.loads(output) if output else {}

    def wait_ready(self, timeout=20):
        def ready():
            try:
                return self.run("ping").get("daemon") == "ok"
            except (subprocess.CalledProcessError, json.JSONDecodeError, OSError):
                return False

        wait_until(ready, "qwed readiness", timeout)

    def create_session(self):
        created = self.run(
            "session",
            "create",
            "service-session",
            "sh",
            "--",
            "-c",
            "sleep 60",
        )
        assert created["running"] is True, created
        return created

    def status(self):
        return self.run("status")

    def session_status(self):
        return self.run("session", "status", "service-session")


class SystemdService:
    def __init__(self, root, qwed, socket_path, store_path):
        self.unit = f"qwe-stage4-lifecycle-{os.getpid()}.service"
        self.path = Path.home() / ".config/systemd/user" / self.unit
        self.root = root
        self.qwed = qwed
        self.socket_path = socket_path
        self.store_path = store_path

    def install(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(
            "\n".join(
                [
                    "[Unit]",
                    "Description=QWE Stage 4 lifecycle probe",
                    "",
                    "[Service]",
                    "Type=simple",
                    f'ExecStart="{self.qwed}" --socket "{self.socket_path}" '
                    f'--store "{self.store_path}"',
                    "Restart=on-failure",
                    "RestartSec=1",
                    "KillMode=process",
                    "TimeoutStopSec=10",
                    "",
                ]
            ),
            encoding="utf-8",
        )
        run(["systemctl", "--user", "daemon-reload"])
        run(["systemctl", "--user", "start", self.unit])

    def main_pid(self):
        output = run(
            ["systemctl", "--user", "show", self.unit, "-p", "MainPID", "--value"]
        ).stdout.strip()
        return int(output) if output.isdigit() else 0

    def stop(self):
        run(["systemctl", "--user", "stop", self.unit], check=False)

    def uninstall(self):
        self.stop()
        self.path.unlink(missing_ok=True)
        run(["systemctl", "--user", "daemon-reload"], check=False)


class LaunchdService:
    def __init__(self, root, qwed, socket_path, store_path):
        self.label = f"com.qwe.stage4.lifecycle.{os.getpid()}"
        self.path = root / f"{self.label}.plist"
        self.root = root
        self.qwed = qwed
        self.socket_path = socket_path
        self.store_path = store_path
        self.domain = f"gui/{os.getuid()}"

    def install(self):
        plist = {
            "Label": self.label,
            "ProgramArguments": [
                str(self.qwed),
                "--socket",
                str(self.socket_path),
                "--store",
                str(self.store_path),
            ],
            "RunAtLoad": True,
            "KeepAlive": {"SuccessfulExit": False},
            "ProcessType": "Interactive",
            "ThrottleInterval": 1,
            "StandardOutPath": str(self.root / "launchd.out.log"),
            "StandardErrorPath": str(self.root / "launchd.err.log"),
        }
        with self.path.open("wb") as stream:
            plistlib.dump(plist, stream)
        run(["launchctl", "bootstrap", self.domain, self.path])

    def main_pid(self):
        target = f"{self.domain}/{self.label}"
        result = run(["launchctl", "print", target], check=False)
        if result.returncode != 0:
            return 0
        match = re.search(r"^\s*pid = (\d+)$", result.stdout, flags=re.MULTILINE)
        return int(match.group(1)) if match else 0

    def stop(self):
        run(["launchctl", "bootout", f"{self.domain}/{self.label}"], check=False)

    def uninstall(self):
        self.stop()
        self.path.unlink(missing_ok=True)


def probe_service(manager, client, socket_path, worker_pid, task_pid):
    old_qwed = wait_until(
        lambda: manager.main_pid() or None,
        "initial service main PID",
    )
    os.kill(old_qwed, signal.SIGKILL)
    wait_process_gone(old_qwed, "crashed qwed")
    assert process_alive(worker_pid), "worker did not survive qwed SIGKILL"

    def restarted():
        try:
            client.wait_ready(timeout=1)
            pid = manager.main_pid()
            return pid if pid and pid != old_qwed else None
        except RuntimeError:
            return None

    new_qwed = wait_until(restarted, "service-manager qwed restart", 25)
    status = client.session_status()
    assert status["running"] is True, status
    assert status["pid"] == task_pid, status
    assert status["worker_pid"] == worker_pid, status

    manager.stop()
    wait_process_gone(new_qwed, "stopped qwed")
    wait_process_gone(worker_pid, "stopped worker")
    wait_process_gone(task_pid, "stopped task")

    store_path = socket_path.with_suffix(".sqlite")
    assert store_path.exists() and store_path.stat().st_size > 0, (
        f"missing QWE store at {store_path}",
        sorted(path.name for path in socket_path.parent.iterdir()),
    )
    with sqlite3.connect(str(store_path)) as connection:
        session_state = connection.execute(
            "SELECT state FROM sessions WHERE id = 'service-session'"
        ).fetchone()
        task_state = connection.execute(
            "SELECT state FROM tasks WHERE session_id = 'service-session'"
        ).fetchone()
    assert session_state == ("closed",), session_state
    assert task_state == ("stopped",), task_state
    return new_qwed


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--qwe", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()
    socket_path = Path(args.socket).resolve()
    qwed = Path(args.qwed).resolve()
    qwe = Path(args.qwe).resolve()
    store_path = socket_path.with_suffix(".sqlite")
    root = socket_path.parent
    manager = None
    worker_pid = 0
    task_pid = 0
    try:
        client = QweClient(str(qwe), str(socket_path))
        if sys.platform == "linux":
            manager = SystemdService(root, qwed, socket_path, store_path)
        elif sys.platform == "darwin":
            manager = LaunchdService(root, qwed, socket_path, store_path)
        else:
            raise RuntimeError(f"unsupported service platform: {sys.platform}")
        manager.install()
        client.wait_ready()
        created = client.create_session()
        worker_pid = created["worker_pid"]
        task_pid = created["pid"]
        restarted = probe_service(
            manager, client, socket_path, worker_pid, task_pid
        )
        print(
            json.dumps(
                {
                    "case": "SVC-002/SVC-003",
                    "service_manager": manager.__class__.__name__,
                    "worker_survived_qwed_sigkill": "pass",
                    "qwed_restart_adopted_worker": "pass",
                    "graceful_stop_cleaned_worker": "pass",
                    "session_closed": "pass",
                    "task_stopped": "pass",
                    "restarted_qwed_pid": restarted,
                },
                separators=(",", ":"),
            )
        )
    finally:
        if manager is not None:
            manager.uninstall()
            remaining_pid = manager.main_pid()
            if process_alive(remaining_pid):
                try:
                    os.kill(remaining_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        for path in (
            socket_path,
            Path(f"{socket_path}.attach"),
            store_path,
            Path(f"{store_path}-shm"),
            Path(f"{store_path}-wal"),
        ):
            path.unlink(missing_ok=True)
        runtime_dir = Path(f"{socket_path}.runtime")
        if runtime_dir.exists():
            for path in runtime_dir.iterdir():
                path.unlink(missing_ok=True)
            runtime_dir.rmdir()
        for pid in (worker_pid, task_pid):
            if process_alive(pid):
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


if __name__ == "__main__":
    main()
