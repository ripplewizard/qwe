#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
import signal
import socket
import sqlite3
import subprocess
import tarfile
import tempfile
import time
from pathlib import Path


SCHEMA_VERSION = 10
ROOT = Path(__file__).resolve().parents[2]


def run(arguments, *, timeout=30, check=True):
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


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def wait_for_socket(path, process, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            stderr = process.stderr.read().decode(errors="replace")
            raise RuntimeError(f"qwed exited with {process.returncode}: {stderr}")
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.connect(str(path))
            return
        except OSError:
            time.sleep(0.02)
    raise RuntimeError(f"timed out waiting for {path}")


def start_qwed(binary, socket_path, store_path):
    process = subprocess.Popen(
        [
            str(binary),
            "--socket",
            str(socket_path),
            "--store",
            str(store_path),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    wait_for_socket(socket_path, process)
    return process


def stop_qwed(process, *, force=False):
    if process is None or process.poll() is not None:
        return
    try:
        os.killpg(
            process.pid, signal.SIGKILL if force else signal.SIGTERM
        )
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=8)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)


def wait_process_gone(pid, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return
        time.sleep(0.02)
    raise RuntimeError(f"process {pid} remained alive")


def downgrade_to_v9(store_path):
    with sqlite3.connect(store_path) as connection:
        connection.execute("PRAGMA foreign_keys = OFF")
        connection.executescript(
            """
            BEGIN IMMEDIATE;
            CREATE TABLE panes_v9 (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL UNIQUE
                    REFERENCES sessions(id) ON DELETE RESTRICT,
                cpu_seconds INTEGER,
                memory_bytes INTEGER,
                process_limit INTEGER,
                updated_at_ms INTEGER NOT NULL
            );
            INSERT INTO panes_v9 (
                id, session_id, cpu_seconds, memory_bytes,
                process_limit, updated_at_ms
            )
            SELECT
                id, session_id, cpu_seconds, memory_bytes,
                process_limit, updated_at_ms
            FROM panes;
            DROP TABLE panes;
            ALTER TABLE panes_v9 RENAME TO panes;
            PRAGMA user_version = 9;
            COMMIT;
            """
        )
        connection.execute("PRAGMA foreign_keys = ON")


def schema_version(store_path):
    with sqlite3.connect(store_path) as connection:
        return connection.execute("PRAGMA user_version").fetchone()[0]


def verify_archive(archive, expected_version):
    with tempfile.TemporaryDirectory(
        prefix="qwe-release-archive-", dir="/tmp"
    ) as root:
        root = Path(root)
        with tarfile.open(archive, "r:gz") as package:
            members = package.getmembers()
            assert members, "release archive is empty"
            for member in members:
                relative = Path(member.name)
                if relative.is_absolute() or ".." in relative.parts:
                    raise RuntimeError(f"unsafe archive member: {member.name}")
                if member.issym() or member.islnk():
                    raise RuntimeError(
                        f"release archive contains unsupported link: {member.name}"
                    )
            if hasattr(tarfile, "data_filter"):
                package.extractall(root, filter="data")
            else:
                package.extractall(root)

        entries = list(root.iterdir())
        assert len(entries) == 1 and entries[0].is_dir(), entries
        extracted = entries[0]
        manifest = json.loads(
            (extracted / "release.json").read_text(encoding="utf-8")
        )
        assert manifest["version"] == expected_version
        installed = json.loads(
            run(
                [
                    "python3",
                    extracted / "install.py",
                    "--prefix",
                    root / "archive-prefix",
                    "--dry-run",
                ]
            ).stdout
        )
        assert installed["dry_run"] is True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    args = parser.parse_args()
    package = args.package.resolve()
    manifest = json.loads((package / "release.json").read_text(encoding="utf-8"))
    assert manifest["schema"] == "qwe-release/v1"
    assert manifest["version"]
    assert manifest["protocol_version"] == 1
    distribution = manifest["distribution"]
    assert distribution["public_trust"] is False
    assert manifest["signing_mode"] in {"unsigned", "self-signed", "developer-id"}
    if (
        manifest["target"].startswith("macos-")
        and manifest["signing_mode"] != "developer-id"
    ):
        assert package.name.endswith("-local")
    if args.archive is not None:
        verify_archive(args.archive.resolve(), manifest["version"])

    source_markers = (str(ROOT),)
    for path in package.rglob("*"):
        if path.is_file() and path.suffix in {
            ".json",
            ".md",
            ".plist",
            ".service",
            ".sh",
            ".py",
        }:
            content = path.read_text(encoding="utf-8", errors="replace")
            for marker in source_markers:
                assert marker not in content, (path, marker)

    qwed_process = None
    worker_pid = None
    task_pid = None
    with tempfile.TemporaryDirectory(
        prefix="qwe-release-acceptance-", dir="/tmp"
    ) as root:
        root = Path(root)
        prefix = root / "home"
        socket_path = root / "run/qwe.sock"
        socket_path.parent.mkdir(mode=0o700)
        store_path = root / "state/qwe.sqlite"
        store_path.parent.mkdir(mode=0o700)

        install_command = [
            "python3",
            package / "install.py",
            "--prefix",
            prefix,
        ]
        installed = json.loads(run(install_command).stdout)
        assert installed["case"] == "PACK-002"
        bin_dir = prefix / ".local/bin"
        for name in ("qwe", "qwed", "qwe-worker", "qwe-session-exec"):
            binary = bin_dir / name
            assert binary.is_file()
            assert binary.stat().st_mode & 0o111
        service_files = [bin_dir / "qwed-wrapper.sh"]
        if manifest["target"].startswith("macos-"):
            service_files.append(prefix / "Library/LaunchAgents/com.qwe.qwed.plist")
        else:
            service_files.append(prefix / ".config/systemd/user/qwe.service")
        for service_file in service_files:
            content = service_file.read_text(encoding="utf-8")
            assert "REPOSITORY_ROOT" not in content
            assert str(ROOT) not in content

        released = json.loads(run([bin_dir / "qwe"]).stdout)
        assert released["version"] == manifest["version"]
        assert released["protocol"] == manifest["protocol_version"]
        assert manifest["version"] in run([bin_dir / "qwed", "--version"]).stdout
        if manifest["signing_mode"] == "self-signed":
            for name in ("qwe", "qwed", "qwe-worker", "qwe-session-exec"):
                signature = run(
                    ["codesign", "-dv", "--verbose=4", bin_dir / name]
                ).stderr
                assert "Authority=QWE Local Release Signing" in signature
                assert "TeamIdentifier=not set" in signature

        try:
            qwed_process = start_qwed(bin_dir / "qwed", socket_path, store_path)
            created = json.loads(
                run(
                    [
                        bin_dir / "qwe",
                        "--socket",
                        socket_path,
                        "session",
                        "create",
                        "release-upgrade",
                        "sh",
                        "--",
                        "-c",
                        "sleep 60",
                    ]
                ).stdout
            )
            worker_pid = created["worker_pid"]
            task_pid = created["pid"]
            database_digest = file_sha256(store_path)

            os.kill(qwed_process.pid, signal.SIGKILL)
            qwed_process.wait(timeout=8)
            qwed_process = None
            os.kill(worker_pid, 0)

            upgraded = json.loads(run(install_command).stdout)
            assert upgraded["case"] == "PACK-002"
            assert file_sha256(store_path) == database_digest

            qwed_process = start_qwed(bin_dir / "qwed", socket_path, store_path)
            for _ in range(100):
                status = json.loads(
                    run(
                        [
                            bin_dir / "qwe",
                            "--socket",
                            socket_path,
                            "session",
                            "status",
                            "release-upgrade",
                        ]
                    ).stdout
                )
                if status.get("running"):
                    break
                time.sleep(0.02)
            assert status["running"] is True
            assert status["worker_pid"] == worker_pid
            assert status["pid"] == task_pid
            assert schema_version(store_path) == SCHEMA_VERSION

            stop_qwed(qwed_process)
            qwed_process = None
            wait_process_gone(worker_pid)
            wait_process_gone(task_pid)
            worker_pid = None
            task_pid = None

            downgrade_to_v9(store_path)
            assert schema_version(store_path) == 9
            qwed_process = start_qwed(bin_dir / "qwed", socket_path, store_path)
            assert schema_version(store_path) == SCHEMA_VERSION
            run(
                [
                    bin_dir / "qwe",
                    "--socket",
                    socket_path,
                    "ping",
                ]
            )
        finally:
            stop_qwed(qwed_process, force=qwed_process is not None)
            qwed_process = None
            for pid in (worker_pid, task_pid):
                if pid is not None:
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    print(
        json.dumps(
            {
                "case": "PACK-003",
                "package": "pass",
                "archive_extraction": (
                    "pass" if args.archive is not None else "not_requested"
                ),
                "distribution_profile": distribution["profile"],
                "public_trust": distribution["public_trust"],
                "temporary_home_install": "pass",
                "service_assets": "pass",
                "database_preserved_during_reinstall": "pass",
                "worker_adopted_after_restart": "pass",
                "schema_v9_to_v10": "pass",
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
