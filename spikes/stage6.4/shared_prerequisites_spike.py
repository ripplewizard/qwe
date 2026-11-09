#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
import platform
import signal
import shutil
import tempfile
import socket
import sqlite3
import struct
import subprocess
import sys
import threading
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable


PROTOCOL_VERSION = 1
ATTACH_PROTOCOL_VERSION = 1

ATTACH_OPEN = 0x01
ATTACH_ACCEPTED = 0x81
SNAPSHOT = 0x82
OUTPUT = 0x83
PROCESS_EXITED = 0x85
ERROR = 0x87

LEGACY_PRINCIPAL_ID = "principal-legacy-local"


class SpikeFailure(RuntimeError):
    pass


class ControlFailure(SpikeFailure):
    def __init__(self, failure: dict[str, Any]):
        super().__init__(f"{failure.get('code')}: {failure.get('message')}")
        self.code = failure.get("code")


def now_ms() -> int:
    return int(time.time() * 1000)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def read_json(path: Path) -> Any:
    return json.loads(path.read_text())


def receive_exact(stream: socket.socket, count: int) -> bytes:
    chunks: list[bytes] = []
    remaining = count
    while remaining:
        chunk = stream.recv(remaining)
        if not chunk:
            raise EOFError("socket closed while receiving a frame")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def send_control(
    path: Path | str,
    operation: str,
    payload: dict[str, Any] | None = None,
    *,
    idempotency_key: str | None = None,
    timeout: float = 10.0,
) -> dict[str, Any]:
    request = {
        "version": PROTOCOL_VERSION,
        "request_id": f"stage64-{uuid.uuid4()}",
        "idempotency_key": idempotency_key,
        "operation": operation,
        "payload": payload or {},
    }
    encoded = json.dumps(request, separators=(",", ":")).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(timeout)
        stream.connect(str(path))
        stream.sendall(struct.pack(">I", len(encoded)))
        stream.sendall(encoded)
        length = struct.unpack(">I", receive_exact(stream, 4))[0]
        response = json.loads(receive_exact(stream, length))
    if response["version"] != PROTOCOL_VERSION:
        raise SpikeFailure(f"unexpected control protocol version {response['version']}")
    result = response["result"]
    if "Err" in result:
        raise ControlFailure(result["Err"])
    return result["Ok"]


def worker_call(
    path: Path | str,
    operation: str,
    payload: dict[str, Any] | None = None,
    *,
    timeout: float = 5.0,
) -> dict[str, Any]:
    return send_control(path, operation, payload or {}, timeout=timeout)


def receive_attach_frame(stream: socket.socket) -> tuple[int, bytes]:
    frame_type, length = struct.unpack(">BI", receive_exact(stream, 5))
    return frame_type, receive_exact(stream, length)


def send_attach_json(
    stream: socket.socket, frame_type: int, value: dict[str, Any]
) -> None:
    payload = json.dumps(value, separators=(",", ":")).encode()
    stream.sendall(struct.pack(">BI", frame_type, len(payload)))
    stream.sendall(payload)


def decode_attach_json(payload: bytes) -> dict[str, Any]:
    return json.loads(payload)


def decode_output(payload: bytes) -> tuple[int, bytes]:
    if len(payload) < 8:
        raise SpikeFailure("truncated ATTACH output frame")
    return struct.unpack(">Q", payload[:8])[0], payload[8:]


def attach_path(control_path: Path) -> Path:
    return control_path.with_suffix(".a")


def wait_for_socket(
    path: Path, process: subprocess.Popen[Any] | None, timeout: float
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process is not None and process.poll() is not None:
            raise SpikeFailure(f"process exited while waiting for {path}")
        if path.exists():
            try:
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                    stream.settimeout(0.2)
                    stream.connect(str(path))
                return
            except OSError:
                pass
        time.sleep(0.02)
    raise SpikeFailure(f"timed out waiting for {path}")


def wait_until(
    description: str,
    predicate: Callable[[], bool],
    timeout: float,
    interval: float = 0.05,
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(interval)
    raise SpikeFailure(f"timed out waiting for {description}")


def log_tail(path: Path, limit: int = 4000) -> str:
    try:
        return path.read_text(errors="replace")[-limit:]
    except OSError:
        return ""


def pid_alive(pid: int | None) -> bool:
    if not pid:
        return False
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
    except (OSError, IndexError):
        return False
    return bool(fields) and fields[0] != "Z"


def process_rss_bytes(pid: int | None) -> int:
    if not pid:
        return 0
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        pass
    return 0


def read_cgroup_path(pid: int) -> Path:
    contents = Path(f"/proc/{pid}/cgroup").read_text()
    relative = next(
        (
            line.removeprefix("0::")
            for line in contents.splitlines()
            if line.startswith("0::")
        ),
        None,
    )
    if relative is None:
        raise SpikeFailure(f"PID {pid} has no cgroup v2 entry")
    return Path("/sys/fs/cgroup") / relative.lstrip("/")


def read_text(path: Path) -> str | None:
    try:
        return path.read_text().strip()
    except OSError:
        return None


def read_kv_file(path: Path) -> dict[str, int]:
    value: dict[str, int] = {}
    text = read_text(path)
    if text is None:
        return value
    for line in text.splitlines():
        fields = line.split()
        if len(fields) == 2:
            try:
                value[fields[0]] = int(fields[1])
            except ValueError:
                pass
    return value


def cgroup_snapshot(path: Path) -> dict[str, Any]:
    return {
        "path": str(path),
        "memory_max": read_text(path / "memory.max"),
        "memory_high": read_text(path / "memory.high"),
        "memory_swap_max": read_text(path / "memory.swap.max"),
        "pids_max": read_text(path / "pids.max"),
        "cpu_max": read_text(path / "cpu.max"),
        "memory_current": read_text(path / "memory.current"),
        "memory_peak": read_text(path / "memory.peak"),
        "pids_current": read_text(path / "pids.current"),
        "pids_peak": read_text(path / "pids.peak"),
        "memory_events": read_kv_file(path / "memory.events"),
        "pids_events": read_kv_file(path / "pids.events"),
    }


def source_commit(root: Path) -> str:
    try:
        return subprocess.check_output(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def sql_limit(value: int | None) -> str:
    return "max" if value is None else str(value)


def planned_policy(index: int, pane_count: int) -> dict[str, Any]:
    return {
        "memory_high_bytes": 96 * 1024 * 1024 + index * 16 * 1024 * 1024,
        "memory_max_bytes": 128 * 1024 * 1024 + index * 16 * 1024 * 1024,
        "memory_swap_max_bytes": 0,
        "tasks_max": 8 if index == pane_count - 1 else 32 + index * 4,
        "cpu_quota_percent": 20 + index * 5,
        "cpu_seconds": None,
    }


def policy_from_pane_row(row: sqlite3.Row) -> dict[str, Any]:
    return {
        "memory_high_bytes": row["memory_high_bytes"],
        "memory_max_bytes": row["memory_bytes"],
        "memory_swap_max_bytes": row["memory_swap_max_bytes"],
        "tasks_max": row["process_limit"],
        "cpu_quota_percent": row["cpu_quota_percent"],
        "cpu_seconds": row["cpu_seconds"],
    }


def policy_json(policy: dict[str, Any]) -> str:
    return json.dumps(policy, separators=(",", ":"), sort_keys=True)


def backup_database(source: Path, destination: Path) -> None:
    with sqlite3.connect(source) as source_connection:
        with sqlite3.connect(destination) as destination_connection:
            source_connection.backup(destination_connection)


def migrate_database(path: Path, now: int) -> dict[str, Any]:
    connection = sqlite3.connect(path)
    connection.row_factory = sqlite3.Row
    try:
        connection.execute("PRAGMA foreign_keys=OFF")
        version = connection.execute("PRAGMA user_version").fetchone()[0]
        if version != 10:
            raise SpikeFailure(f"expected schema v10, found v{version}")
        sessions = [
            dict(row) for row in connection.execute("SELECT * FROM sessions")
        ]
        panes = [
            dict(row)
            for row in connection.execute(
                """
                SELECT id, session_id, cpu_seconds, memory_high_bytes, memory_bytes,
                       memory_swap_max_bytes, process_limit, cpu_quota_percent,
                       updated_at_ms
                FROM panes
                """
            )
        ]
        tasks = [
            dict(row)
            for row in connection.execute(
                """
                SELECT id, session_id, pid, pgid, start_token, state, exit_code,
                       exit_signal, started_at_ms, ended_at_ms
                FROM tasks
                """
            )
        ]
        if len(sessions) != 1 or len(panes) != 1 or len(tasks) != 1:
            raise SpikeFailure(
                "legacy fixture must contain exactly one Session, Pane, and Task"
            )
        session = sessions[0]
        legacy_pane = panes[0]
        legacy_task = tasks[0]
        legacy_policy = policy_from_pane_row(legacy_pane)
        session_scope_id = str(uuid.uuid4())
        pane_scopes: dict[str, str] = {}
        task_scopes: dict[str, str] = {}

        connection.execute("BEGIN IMMEDIATE")
        connection.execute(
            """
            CREATE TABLE principals (
                id TEXT PRIMARY KEY,
                parent_id TEXT REFERENCES principals(id) ON DELETE RESTRICT,
                epoch INTEGER NOT NULL,
                revoked_at_ms INTEGER,
                created_at_ms INTEGER NOT NULL
            )
            """
        )
        connection.execute(
            """
            INSERT INTO principals
                (id, parent_id, epoch, revoked_at_ms, created_at_ms)
            VALUES (?, NULL, 1, NULL, ?)
            """,
            (LEGACY_PRINCIPAL_ID, now),
        )
        connection.execute(
            """
            CREATE TABLE resource_scopes (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL
                    REFERENCES sessions(id) ON DELETE RESTRICT,
                parent_scope_id TEXT
                    REFERENCES resource_scopes(id) ON DELETE RESTRICT,
                target_kind TEXT NOT NULL
                    CHECK(target_kind IN ('session', 'pane', 'task')),
                target_id TEXT NOT NULL,
                backend TEXT NOT NULL
                    CHECK(backend IN ('linux_cgroup_v2', 'macos_monitored')),
                requested_policy_json TEXT NOT NULL,
                effective_policy_json TEXT,
                lifecycle_state TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL
            )
            """
        )
        connection.execute(
            """
            CREATE UNIQUE INDEX resource_scopes_target
            ON resource_scopes(session_id, target_kind, target_id)
            """
        )
        connection.execute(
            """
            INSERT INTO resource_scopes
                (id, session_id, parent_scope_id, target_kind, target_id, backend,
                 requested_policy_json, effective_policy_json, lifecycle_state,
                 updated_at_ms)
            VALUES (?, ?, NULL, 'session', ?, 'linux_cgroup_v2', ?, ?, 'active', ?)
            """,
            (
                session_scope_id,
                session["id"],
                session["id"],
                policy_json(legacy_policy),
                policy_json(legacy_policy),
                now,
            ),
        )
        for pane in panes:
            scope_id = str(uuid.uuid4())
            pane_scopes[pane["id"]] = scope_id
            policy = policy_from_pane_row(pane)
            connection.execute(
                """
                INSERT INTO resource_scopes
                    (id, session_id, parent_scope_id, target_kind, target_id,
                     backend, requested_policy_json, effective_policy_json,
                     lifecycle_state, updated_at_ms)
                VALUES (?, ?, ?, 'pane', ?, 'linux_cgroup_v2', ?, ?, 'active', ?)
                """,
                (
                    scope_id,
                    pane["session_id"],
                    session_scope_id,
                    pane["id"],
                    policy_json(policy),
                    policy_json(policy),
                    now,
                ),
            )
        for task in tasks:
            scope_id = str(uuid.uuid4())
            task_scopes[task["id"]] = scope_id
            connection.execute(
                """
                INSERT INTO resource_scopes
                    (id, session_id, parent_scope_id, target_kind, target_id,
                     backend, requested_policy_json, effective_policy_json,
                     lifecycle_state, updated_at_ms)
                VALUES (?, ?, ?, 'task', ?, 'linux_cgroup_v2', '{}', '{}',
                        'active', ?)
                """,
                (
                    scope_id,
                    task["session_id"],
                    pane_scopes[legacy_pane["id"]],
                    task["id"],
                    now,
                ),
            )
        connection.execute("ALTER TABLE sessions ADD COLUMN owner_principal_id TEXT")
        connection.execute(
            "UPDATE sessions SET owner_principal_id = ?", (LEGACY_PRINCIPAL_ID,)
        )

        connection.execute("PRAGMA legacy_alter_table=ON")
        connection.execute("DROP INDEX IF EXISTS tasks_by_session")
        connection.execute("ALTER TABLE tasks RENAME TO tasks_v10")
        connection.execute(
            """
            CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL
                    REFERENCES sessions(id) ON DELETE RESTRICT,
                pane_id TEXT NOT NULL
                    REFERENCES panes(id) ON DELETE RESTRICT,
                resource_scope_id TEXT NOT NULL
                    REFERENCES resource_scopes(id) ON DELETE RESTRICT,
                pid INTEGER NOT NULL,
                pgid INTEGER NOT NULL,
                start_token TEXT NOT NULL,
                state TEXT NOT NULL,
                exit_code INTEGER,
                exit_signal TEXT,
                started_at_ms INTEGER NOT NULL,
                ended_at_ms INTEGER
            )
            """
        )
        for task in tasks:
            connection.execute(
                """
                INSERT INTO tasks
                    (id, session_id, pane_id, resource_scope_id, pid, pgid,
                     start_token, state, exit_code, exit_signal, started_at_ms,
                     ended_at_ms)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    task["id"],
                    task["session_id"],
                    legacy_pane["id"],
                    task_scopes[task["id"]],
                    task["pid"],
                    task["pgid"],
                    task["start_token"],
                    task["state"],
                    task["exit_code"],
                    task["exit_signal"],
                    task["started_at_ms"],
                    task["ended_at_ms"],
                ),
            )
        connection.execute("DROP TABLE tasks_v10")
        connection.execute("CREATE INDEX tasks_by_session ON tasks(session_id)")
        connection.execute("CREATE INDEX tasks_by_pane ON tasks(pane_id)")

        connection.execute("ALTER TABLE panes RENAME TO panes_v10")
        connection.execute(
            """
            CREATE TABLE panes (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL
                    REFERENCES sessions(id) ON DELETE RESTRICT,
                resource_scope_id TEXT NOT NULL
                    REFERENCES resource_scopes(id) ON DELETE RESTRICT,
                cpu_seconds INTEGER,
                memory_high_bytes INTEGER,
                memory_bytes INTEGER,
                memory_swap_max_bytes INTEGER,
                process_limit INTEGER,
                cpu_quota_percent INTEGER,
                updated_at_ms INTEGER NOT NULL
            )
            """
        )
        for pane in panes:
            connection.execute(
                """
                INSERT INTO panes
                    (id, session_id, resource_scope_id, cpu_seconds,
                     memory_high_bytes, memory_bytes, memory_swap_max_bytes,
                     process_limit, cpu_quota_percent, updated_at_ms)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    pane["id"],
                    pane["session_id"],
                    pane_scopes[pane["id"]],
                    pane["cpu_seconds"],
                    pane["memory_high_bytes"],
                    pane["memory_bytes"],
                    pane["memory_swap_max_bytes"],
                    pane["process_limit"],
                    pane["cpu_quota_percent"],
                    pane["updated_at_ms"],
                ),
            )
        connection.execute("DROP TABLE panes_v10")
        connection.execute("CREATE INDEX panes_by_session ON panes(session_id)")
        connection.execute("PRAGMA legacy_alter_table=OFF")
        connection.execute(
            """
            CREATE TABLE pane_workers (
                pane_id TEXT PRIMARY KEY
                    REFERENCES panes(id) ON DELETE RESTRICT,
                worker_session_id TEXT NOT NULL UNIQUE,
                worker_pid INTEGER,
                worker_pgid INTEGER,
                worker_start_token TEXT,
                control_socket TEXT NOT NULL,
                attach_socket TEXT NOT NULL,
                resource_scope_id TEXT NOT NULL
                    REFERENCES resource_scopes(id) ON DELETE RESTRICT,
                health TEXT NOT NULL,
                lifecycle_state TEXT NOT NULL,
                adopted_at_ms INTEGER,
                updated_at_ms INTEGER NOT NULL
            )
            """
        )
        connection.execute(
            """
            CREATE TABLE channel_grants (
                id TEXT PRIMARY KEY,
                principal_id TEXT NOT NULL
                    REFERENCES principals(id) ON DELETE RESTRICT,
                session_id TEXT NOT NULL
                    REFERENCES sessions(id) ON DELETE RESTRICT,
                pane_id TEXT
                    REFERENCES panes(id) ON DELETE RESTRICT,
                profile TEXT NOT NULL,
                projection TEXT NOT NULL,
                can_write INTEGER NOT NULL CHECK(can_write IN (0, 1)),
                auth_context TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                revoked_at_ms INTEGER
            )
            """
        )
        connection.execute("PRAGMA user_version = 11")
        connection.commit()
        connection.execute("PRAGMA foreign_keys=ON")
        violations = connection.execute("PRAGMA foreign_key_check").fetchall()
        if violations:
            raise SpikeFailure(f"foreign-key violations after migration: {violations}")
        migrated_version = connection.execute("PRAGMA user_version").fetchone()[0]
        migrated_session = connection.execute(
            "SELECT * FROM sessions WHERE id = ?", (session["id"],)
        ).fetchone()
        migrated_pane = connection.execute(
            "SELECT * FROM panes WHERE id = ?", (legacy_pane["id"],)
        ).fetchone()
        migrated_task = connection.execute(
            "SELECT * FROM tasks WHERE id = ?", (legacy_task["id"],)
        ).fetchone()
        if (
            migrated_version != 11
            or migrated_session is None
            or migrated_pane is None
            or migrated_task is None
        ):
            raise SpikeFailure("migration did not preserve the legacy Session/Pane/Task")
        if policy_from_pane_row(migrated_pane) != legacy_policy:
            raise SpikeFailure("migration changed the legacy resource policy")
        if migrated_task["pane_id"] != legacy_pane["id"]:
            raise SpikeFailure("migrated Task did not retain its Pane")
        return {
            "legacy_schema_version": version,
            "migrated_schema_version": migrated_version,
            "legacy_session_id": session["id"],
            "legacy_pane_id": legacy_pane["id"],
            "legacy_task_id": legacy_task["id"],
            "legacy_resource_policy": legacy_policy,
            "session_scope_id": session_scope_id,
            "pane_scope_id": pane_scopes[legacy_pane["id"]],
            "task_scope_id": task_scopes[legacy_task["id"]],
            "foreign_key_violations": len(violations),
        }
    finally:
        connection.close()


def add_extra_panes(
    path: Path,
    session_id: str,
    planned_panes: list[dict[str, Any]],
    now: int,
) -> None:
    connection = sqlite3.connect(path)
    connection.row_factory = sqlite3.Row
    try:
        session_scope_id = connection.execute(
            """
            SELECT id FROM resource_scopes
            WHERE session_id = ? AND target_kind = 'session'
            """,
            (session_id,),
        ).fetchone()["id"]
        for pane in planned_panes:
            policy = pane["policy"]
            connection.execute(
                """
                UPDATE panes SET
                    cpu_seconds = ?, memory_high_bytes = ?, memory_bytes = ?,
                    memory_swap_max_bytes = ?, process_limit = ?,
                    cpu_quota_percent = ?, updated_at_ms = ?
                WHERE id = ?
                """,
                (
                    policy["cpu_seconds"],
                    policy["memory_high_bytes"],
                    policy["memory_max_bytes"],
                    policy["memory_swap_max_bytes"],
                    policy["tasks_max"],
                    policy["cpu_quota_percent"],
                    now,
                    pane["pane_id"],
                ),
            )
            existing = connection.execute(
                """
                SELECT id FROM resource_scopes
                WHERE session_id = ? AND target_kind = 'pane' AND target_id = ?
                """,
                (session_id, pane["pane_id"]),
            ).fetchone()
            if existing is None:
                scope_id = str(uuid.uuid4())
                connection.execute(
                    """
                    INSERT INTO resource_scopes
                        (id, session_id, parent_scope_id, target_kind, target_id,
                         backend, requested_policy_json, effective_policy_json,
                         lifecycle_state, updated_at_ms)
                    VALUES (?, ?, ?, 'pane', ?, 'linux_cgroup_v2', ?, ?, 'active', ?)
                    """,
                    (
                        scope_id,
                        session_id,
                        session_scope_id,
                        pane["pane_id"],
                        policy_json(policy),
                        policy_json(policy),
                        now,
                    ),
                )
                connection.execute(
                    """
                    INSERT INTO panes
                        (id, session_id, resource_scope_id, cpu_seconds,
                         memory_high_bytes, memory_bytes, memory_swap_max_bytes,
                         process_limit, cpu_quota_percent, updated_at_ms)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        pane["pane_id"],
                        session_id,
                        scope_id,
                        policy["cpu_seconds"],
                        policy["memory_high_bytes"],
                        policy["memory_max_bytes"],
                        policy["memory_swap_max_bytes"],
                        policy["tasks_max"],
                        policy["cpu_quota_percent"],
                        now,
                    ),
                )
                pane["scope_id"] = scope_id
            else:
                pane["scope_id"] = existing["id"]
                connection.execute(
                    """
                    UPDATE resource_scopes SET
                        requested_policy_json = ?,
                        effective_policy_json = ?,
                        updated_at_ms = ?
                    WHERE id = ?
                    """,
                    (policy_json(policy), policy_json(policy), now, pane["scope_id"]),
                )
        count = connection.execute(
            "SELECT COUNT(*) FROM panes WHERE session_id = ?", (session_id,)
        ).fetchone()[0]
        if count != len(planned_panes):
            raise SpikeFailure(
                f"multi-Pane migration contains {count} Panes, "
                f"expected {len(planned_panes)}"
            )
        connection.commit()
    finally:
        connection.close()


def start_qwed(
    binary: Path, socket_path: Path, store_path: Path, log_path: Path
) -> tuple[subprocess.Popen[Any], Any]:
    log = log_path.open("ab", buffering=0)
    process = subprocess.Popen(
        [str(binary), "--socket", str(socket_path), "--store", str(store_path)],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=log,
        start_new_session=True,
    )
    try:
        wait_for_socket(socket_path, process, 10)
    except Exception:
        process.kill()
        process.wait(timeout=5)
        raise
    return process, log


def stop_process(process: subprocess.Popen[Any] | None, timeout: float = 5.0) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=timeout)


def prepare_run(
    run_dir: Path,
    qwed: Path,
    worker: Path,
    pane_count: int,
    repo_root: Path,
) -> dict[str, Any]:
    if not 2 <= pane_count <= 4:
        raise SpikeFailure("Stage 6.4 requires 2-4 Panes")
    run_dir.mkdir(parents=True, exist_ok=True)
    v0_dir = run_dir / "v0"
    v0_dir.mkdir()
    temp_root = Path(tempfile.mkdtemp(prefix="q64-"))
    v0_socket = temp_root / "q.sock"
    v0_store = v0_dir / "v0.sqlite"
    shared_store = run_dir / "shared.sqlite"
    runtime_dir = temp_root / "runtime"
    runtime_dir.mkdir(mode=0o700)
    session_id = f"s64-{uuid.uuid4().hex[:10]}"
    source_policy = {
        "memory_high_bytes": 96 * 1024 * 1024,
        "memory_max_bytes": 128 * 1024 * 1024,
        "memory_swap_max_bytes": 0,
        "tasks_max": 32,
        "cpu_quota_percent": 20,
    }
    qwed_process = None
    qwed_log = None
    try:
        qwed_process, qwed_log = start_qwed(
            qwed, v0_socket, v0_store, v0_dir / "qwed.log"
        )
        created = send_control(
            v0_socket,
            "SESSION_CREATE",
            {
                "session_id": session_id,
                "program": "sh",
                "args": [],
                "resource_policy": source_policy,
            },
            idempotency_key=f"stage64-create-{session_id}",
        )
        send_control(
            v0_socket,
            "SERVER_KILL",
            {"confirm": True},
            idempotency_key=f"stage64-kill-{session_id}",
        )
        qwed_process.wait(timeout=10)
        wait_until("V0 worker teardown", lambda: not pid_alive(created["worker_pid"]), 5)
        backup_database(v0_store, shared_store)
        migration = migrate_database(shared_store, now_ms())
        planned_panes = []
        for index in range(pane_count):
            pane_id = (
                migration["legacy_pane_id"] if index == 0 else str(uuid.uuid4())
            )
            planned_panes.append(
                {
                    "index": index,
                    "pane_id": pane_id,
                    "worker_session_id": f"w{index}-{uuid.uuid4().hex[:8]}",
                    "policy": planned_policy(index, pane_count),
                    "scope_id": (
                        migration["pane_scope_id"] if index == 0 else None
                    ),
                }
            )
        add_extra_panes(shared_store, session_id, planned_panes, now_ms())
        migration["pane_count_after_insert"] = len(planned_panes)
        manifest = {
            "stage": "6.4",
            "started_at": utc_now(),
            "source_commit": source_commit(repo_root),
            "run_dir": str(run_dir),
            "repo_root": str(repo_root),
            "qwed": str(qwed),
            "worker": str(worker),
            "session_id": session_id,
            "v0_session_id": created["session_id"],
            "v0_pane_id": created["pane_id"],
            "v0_task_id": created["task_id"],
            "v0_worker_pid": created["worker_pid"],
            "v0_store": str(v0_store),
            "shared_store": str(shared_store),
            "temp_root": str(temp_root),
            "runtime_dir": str(runtime_dir),
            "planned_panes": planned_panes,
            "migration": migration,
        }
        write_json(run_dir / "manifest.json", manifest)
        result = {
            "status": "pass",
            "migration": migration,
            "manifest": str(run_dir / "manifest.json"),
            "checks": {
                "v0_database_created": True,
                "v10_to_v11_preserved_state": True,
                "multiple_panes_inserted": True,
            },
        }
        write_json(run_dir / "phases" / "prepare.json", result)
        return result
    finally:
        if qwed_log is not None:
            qwed_log.close()
        if qwed_process is not None and qwed_process.poll() is None:
            stop_process(qwed_process)


def worker_arguments(
    worker: Path,
    control_socket: Path,
    worker_session_id: str,
    policy: dict[str, Any],
) -> list[str]:
    arguments = [
        str(worker),
        "--worker-socket",
        str(control_socket),
        "--worker-session-id",
        worker_session_id,
        "--worker-program",
        "sh",
        "--worker-systemd-scope",
    ]
    mappings = [
        ("memory_high_bytes", "--worker-memory-high-bytes"),
        ("memory_max_bytes", "--worker-memory-bytes"),
        ("memory_swap_max_bytes", "--worker-memory-swap-max-bytes"),
        ("tasks_max", "--worker-tasks-max"),
        ("cpu_quota_percent", "--worker-cpu-quota-percent"),
        ("cpu_seconds", "--worker-cpu-seconds"),
    ]
    for key, flag in mappings:
        if policy.get(key) is not None:
            arguments.extend([flag, str(policy[key])])
    return arguments


def persist_worker(
    manifest: dict[str, Any],
    pane: dict[str, Any],
    status: dict[str, Any],
    control_socket: Path,
) -> str:
    now = now_ms()
    connection = sqlite3.connect(manifest["shared_store"])
    connection.row_factory = sqlite3.Row
    try:
        worker_identity = status["worker_identity"]
        task_identity = status["process_identity"]
        task_scope = connection.execute(
            """
            SELECT id FROM resource_scopes
            WHERE session_id = ? AND target_kind = 'task' AND target_id = ?
            """,
            (manifest["session_id"], pane["pane_id"]),
        ).fetchone()
        if task_scope is None:
            task_scope_id = str(uuid.uuid4())
            connection.execute(
                """
                INSERT INTO resource_scopes
                    (id, session_id, parent_scope_id, target_kind, target_id,
                     backend, requested_policy_json, effective_policy_json,
                     lifecycle_state, updated_at_ms)
                VALUES (?, ?, ?, 'task', ?, 'linux_cgroup_v2', '{}', '{}',
                        'active', ?)
                """,
                (
                    task_scope_id,
                    manifest["session_id"],
                    pane["scope_id"],
                    pane["pane_id"],
                    now,
                ),
            )
        else:
            task_scope_id = task_scope["id"]
        pane["task_scope_id"] = task_scope_id
        state = "running" if status.get("running") else "exited"
        if pane["index"] == 0:
            task_id = manifest["migration"]["legacy_task_id"]
            connection.execute(
                """
                UPDATE tasks SET
                    pane_id = ?, resource_scope_id = ?, pid = ?, pgid = ?,
                    start_token = ?, state = ?, started_at_ms = ?
                WHERE id = ?
                """,
                (
                    pane["pane_id"],
                    task_scope_id,
                    task_identity["pid"],
                    task_identity["pgid"],
                    task_identity["start_token"],
                    state,
                    now,
                    task_id,
                ),
            )
        else:
            task_id = str(uuid.uuid4())
            connection.execute(
                """
                INSERT INTO tasks
                    (id, session_id, pane_id, resource_scope_id, pid, pgid,
                     start_token, state, exit_code, exit_signal, started_at_ms,
                     ended_at_ms)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, NULL)
                """,
                (
                    task_id,
                    manifest["session_id"],
                    pane["pane_id"],
                    task_scope_id,
                    task_identity["pid"],
                    task_identity["pgid"],
                    task_identity["start_token"],
                    state,
                    now,
                ),
            )
        connection.execute(
            """
            INSERT INTO pane_workers
                (pane_id, worker_session_id, worker_pid, worker_pgid,
                 worker_start_token, control_socket, attach_socket,
                 resource_scope_id, health, lifecycle_state, adopted_at_ms,
                 updated_at_ms)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'healthy', 'running', ?, ?)
            ON CONFLICT(pane_id) DO UPDATE SET
                worker_session_id = excluded.worker_session_id,
                worker_pid = excluded.worker_pid,
                worker_pgid = excluded.worker_pgid,
                worker_start_token = excluded.worker_start_token,
                control_socket = excluded.control_socket,
                attach_socket = excluded.attach_socket,
                resource_scope_id = excluded.resource_scope_id,
                health = excluded.health,
                lifecycle_state = excluded.lifecycle_state,
                adopted_at_ms = excluded.adopted_at_ms,
                updated_at_ms = excluded.updated_at_ms
            """,
            (
                pane["pane_id"],
                pane["worker_session_id"],
                worker_identity["pid"],
                worker_identity["pgid"],
                worker_identity["start_token"],
                str(control_socket),
                str(attach_path(control_socket)),
                pane["scope_id"],
                now,
                now,
            ),
        )
        connection.commit()
        pane["task_id"] = task_id
        return task_id
    finally:
        connection.close()


def start_workers(manifest_path: Path) -> dict[str, Any]:
    manifest = read_json(manifest_path)
    manifest["workers"] = []
    processes: list[subprocess.Popen[Any]] = []
    try:
        for pane in manifest["planned_panes"]:
            worker_session_id = pane["worker_session_id"]
            control_socket = Path(manifest["runtime_dir"]) / (
                worker_session_id.encode().hex() + ".sock"
            )
            worker_log = Path(manifest["run_dir"]) / f"worker-{pane['index']}.log"
            started = time.perf_counter()
            with worker_log.open("ab", buffering=0) as log:
                process = subprocess.Popen(
                    worker_arguments(
                        Path(manifest["worker"]),
                        control_socket,
                        worker_session_id,
                        pane["policy"],
                    ),
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=log,
                    start_new_session=True,
                )
            processes.append(process)
            try:
                wait_for_socket(control_socket, process, 10)
                status = worker_call(control_socket, "WORKER_HELLO", timeout=5)
            except Exception as failure:
                raise SpikeFailure(
                    f"Pane {pane['index']} worker failed to start: {failure}\n"
                    f"{log_tail(worker_log)}"
                ) from failure
            elapsed_ms = (time.perf_counter() - started) * 1000
            if status.get("session_id") != worker_session_id:
                raise SpikeFailure(
                    f"Pane {pane['index']} worker reported {status.get('session_id')}"
                )
            if status.get("running") is not True:
                raise SpikeFailure(f"Pane {pane['index']} shell is not running")
            if status.get("resource_policy") != pane["policy"]:
                raise SpikeFailure(
                    f"Pane {pane['index']} policy mismatch: "
                    f"{status.get('resource_policy')} != {pane['policy']}"
                )
            persist_worker(manifest, pane, status, control_socket)
            pane["control_socket"] = str(control_socket)
            pane["attach_socket"] = str(attach_path(control_socket))
            pane["startup_ms"] = elapsed_ms
            pane["worker_pid"] = status["worker_identity"]["pid"]
            pane["worker_identity"] = status["worker_identity"]
            pane["task_pid"] = status["process_identity"]["pid"]
            pane["task_pgid"] = status["process_identity"]["pgid"]
            pane["task_identity"] = status["process_identity"]
            pane["cgroup_path"] = str(read_cgroup_path(pane["task_pid"]))
            manifest["workers"].append(pane)
        write_json(manifest_path, manifest)
        result = {
            "status": "pass",
            "pane_count": len(manifest["workers"]),
            "startup_ms": {
                str(pane["index"]): pane["startup_ms"]
                for pane in manifest["workers"]
            },
            "checks": {
                "all_pane_workers_started": True,
                "one_worker_per_pane": True,
                "pane_resource_policies_persisted": True,
            },
        }
        write_json(Path(manifest["run_dir"]) / "phases" / "start.json", result)
        return result
    except Exception:
        for pane in manifest["workers"]:
            try:
                os.killpg(pane["task_pgid"], signal.SIGKILL)
            except (OSError, KeyError):
                pass
            try:
                os.kill(pane["worker_pid"], signal.SIGKILL)
            except (OSError, KeyError):
                pass
        for process in processes:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
        raise


def probe_read_only_rejection(worker_session_id: str, control: Path) -> dict[str, Any]:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(3)
        stream.connect(str(attach_path(control)))
        send_attach_json(
            stream,
            ATTACH_OPEN,
            {
                "protocol_version": ATTACH_PROTOCOL_VERSION,
                "session_id": worker_session_id,
                "owner": "stage64-observer-direct",
                "rows": 24,
                "columns": 80,
                "mode": "read_only",
            },
        )
        frame_type, payload = receive_attach_frame(stream)
    if frame_type != ERROR:
        raise SpikeFailure(
            f"native read_only Attach unexpectedly returned 0x{frame_type:02x}"
        )
    failure = decode_attach_json(payload)
    if failure.get("code") != "unsupported_mode":
        raise SpikeFailure(f"unexpected native read_only error: {failure}")
    return failure


def probe_exclusive_attach(pane: dict[str, Any]) -> dict[str, Any]:
    marker = f"STAGE64_TARGET_{pane['index']}_{uuid.uuid4().hex[:8]}"
    raw_output = bytearray()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(pane["attach_socket"])
        send_attach_json(
            stream,
            ATTACH_OPEN,
            {
                "protocol_version": ATTACH_PROTOCOL_VERSION,
                "session_id": pane["worker_session_id"],
                "owner": "stage64-target-attach",
                "rows": 32,
                "columns": 100,
                "mode": "exclusive",
            },
        )
        frame_type, payload = receive_attach_frame(stream)
        if frame_type != ATTACH_ACCEPTED:
            raise SpikeFailure(
                f"target Attach failed with 0x{frame_type:02x}: "
                f"{payload.decode(errors='replace')}"
            )
        accepted = decode_attach_json(payload)
        frame_type, payload = receive_attach_frame(stream)
        if frame_type != SNAPSHOT:
            raise SpikeFailure("target Attach did not return SNAPSHOT")
        snapshot = decode_attach_json(payload)
        worker_call(
            pane["control_socket"],
            "SESSION_INPUT",
            {
                "session_id": pane["worker_session_id"],
                "data": f"printf '{marker}\\n'\r",
            },
        )
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            frame_type, payload = receive_attach_frame(stream)
            if frame_type == OUTPUT:
                _, data = decode_output(payload)
                raw_output.extend(data)
                if marker.encode() in raw_output:
                    break
            elif frame_type in (PROCESS_EXITED, ERROR):
                raise SpikeFailure(
                    f"target Attach terminated before marker: "
                    f"{payload.decode(errors='replace')}"
                )
        else:
            raise SpikeFailure("target Attach timed out waiting for marker")
    if accepted["session_id"] != pane["worker_session_id"]:
        raise SpikeFailure("target Attach returned the wrong worker identity")
    return {
        "pane_index": pane["index"],
        "worker_session_id": pane["worker_session_id"],
        "accepted_owner": accepted["owner"],
        "snapshot_rows": snapshot["rows"],
        "snapshot_columns": snapshot["columns"],
        "marker": marker,
        "output_bytes": len(raw_output),
    }


def read_line(stream: socket.socket, buffer: bytearray) -> dict[str, Any]:
    while b"\n" not in buffer:
        chunk = stream.recv(65536)
        if not chunk:
            raise EOFError("observer bridge closed")
        buffer.extend(chunk)
    line, _, remaining = buffer.partition(b"\n")
    buffer[:] = remaining
    return json.loads(line)


def send_line(stream: socket.socket, value: dict[str, Any]) -> None:
    stream.sendall(json.dumps(value, separators=(",", ":")).encode() + b"\n")


def probe_observer_projection(
    pane: dict[str, Any],
    observer_socket: Path,
    forbidden_marker: Path,
) -> dict[str, Any]:
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(str(observer_socket))
    listener.listen(1)
    listener.settimeout(5)
    errors: list[BaseException] = []
    completed = threading.Event()
    marker = f"STAGE64_OBSERVER_{uuid.uuid4().hex[:8]}"

    def bridge() -> None:
        try:
            client, _ = listener.accept()
            client.settimeout(5)
            buffer = bytearray()
            request = read_line(client, buffer)
            if request.get("op") != "open" or request.get("pane_id") != pane["pane_id"]:
                send_line(client, {"type": "error", "code": "wrong_target"})
                return
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as worker_stream:
                worker_stream.settimeout(5)
                worker_stream.connect(pane["attach_socket"])
                send_attach_json(
                    worker_stream,
                    ATTACH_OPEN,
                    {
                        "protocol_version": ATTACH_PROTOCOL_VERSION,
                        "session_id": pane["worker_session_id"],
                        "owner": "stage64-qwed-observer-bridge",
                        "rows": 32,
                        "columns": 100,
                        "mode": "exclusive",
                    },
                )
                frame_type, payload = receive_attach_frame(worker_stream)
                if frame_type != ATTACH_ACCEPTED:
                    raise SpikeFailure("observer bridge Attach was rejected")
                accepted = decode_attach_json(payload)
                frame_type, payload = receive_attach_frame(worker_stream)
                if frame_type != SNAPSHOT:
                    raise SpikeFailure("observer bridge Attach had no SNAPSHOT")
                snapshot = decode_attach_json(payload)
                send_line(
                    client,
                    {
                        "type": "accepted",
                        "profile": "human_observer",
                        "projection": "structured",
                        "can_write": False,
                        "generation": accepted["generation"],
                        "snapshot": snapshot,
                    },
                )
                write_attempt = read_line(client, buffer)
                if write_attempt.get("op") == "input":
                    send_line(
                        client,
                        {
                            "type": "error",
                            "code": "read_only",
                            "message": "observer profile has no INPUT capability",
                        },
                    )
                else:
                    send_line(client, {"type": "error", "code": "unknown_operation"})
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    frame_type, payload = receive_attach_frame(worker_stream)
                    if frame_type == OUTPUT:
                        offset, data = decode_output(payload)
                        text = data.decode(errors="replace")
                        send_line(
                            client,
                            {"type": "output", "stream_offset": offset, "text": text},
                        )
                        if marker in text:
                            break
                    elif frame_type in (PROCESS_EXITED, ERROR):
                        raise SpikeFailure("observer bridge terminal frame")
                else:
                    raise SpikeFailure("observer bridge timed out waiting for output")
            client.close()
        except BaseException as failure:
            errors.append(failure)
        finally:
            completed.set()

    thread = threading.Thread(target=bridge, daemon=True)
    thread.start()
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as observer:
            observer.settimeout(5)
            observer.connect(str(observer_socket))
            buffer = bytearray()
            send_line(
                observer,
                {
                    "op": "open",
                    "pane_id": pane["pane_id"],
                    "profile": "human_observer",
                },
            )
            accepted = read_line(observer, buffer)
            if accepted.get("type") != "accepted" or accepted.get("can_write") is not False:
                raise SpikeFailure(f"observer was not admitted read-only: {accepted}")
            send_line(
                observer,
                {"op": "input", "data": f"touch {forbidden_marker}\r"},
            )
            rejected = read_line(observer, buffer)
            if rejected.get("type") != "error" or rejected.get("code") != "read_only":
                raise SpikeFailure(f"observer INPUT was not rejected: {rejected}")
            worker_call(
                pane["control_socket"],
                "SESSION_INPUT",
                {
                    "session_id": pane["worker_session_id"],
                    "data": f"printf '{marker}\\n'\r",
                },
            )
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                event = read_line(observer, buffer)
                if event.get("type") == "output" and marker in event.get("text", ""):
                    break
            else:
                raise SpikeFailure("observer did not receive live Pane output")
    finally:
        listener.close()
        completed.wait(timeout=5)
        thread.join(timeout=1)
    if errors:
        raise SpikeFailure(f"observer bridge failed: {errors[0]}")
    if forbidden_marker.exists():
        raise SpikeFailure("observer INPUT reached the Pane shell")
    return {
        "pane_index": pane["index"],
        "profile": "human_observer",
        "projection": "structured",
        "can_write": False,
        "write_attempt_rejected": True,
        "live_output_observed": True,
    }


def process_measurements(workers: list[dict[str, Any]]) -> dict[str, Any]:
    worker_pids = [pane["worker_pid"] for pane in workers]
    task_pids = [pane["task_pid"] for pane in workers]
    return {
        "worker_processes": sum(pid_alive(pid) for pid in worker_pids),
        "task_processes": sum(pid_alive(pid) for pid in task_pids),
        "worker_rss_bytes": sum(process_rss_bytes(pid) for pid in worker_pids),
        "task_rss_bytes": sum(process_rss_bytes(pid) for pid in task_pids),
    }


def verify_cgroups(workers: list[dict[str, Any]]) -> dict[str, Any]:
    snapshots = []
    paths = set()
    for pane in workers:
        path = Path(pane["cgroup_path"])
        if path in paths:
            raise SpikeFailure(f"Pane cgroup is not unique: {path}")
        paths.add(path)
        snapshot = cgroup_snapshot(path)
        snapshot["pane_index"] = pane["index"]
        policy = pane["policy"]
        expected = {
            "memory_max": sql_limit(policy["memory_max_bytes"]),
            "memory_high": sql_limit(policy["memory_high_bytes"]),
            "memory_swap_max": sql_limit(policy["memory_swap_max_bytes"]),
            "pids_max": sql_limit(policy["tasks_max"]),
        }
        for key, value in expected.items():
            if snapshot[key] != value:
                raise SpikeFailure(
                    f"Pane {pane['index']} effective {key} is "
                    f"{snapshot[key]}, expected {value}"
                )
        expected_cpu = f"{policy['cpu_quota_percent'] * 1000} 100000"
        if snapshot["cpu_max"] != expected_cpu:
            raise SpikeFailure(
                f"Pane {pane['index']} effective cpu.max is "
                f"{snapshot['cpu_max']}, expected {expected_cpu}"
            )
        snapshots.append(snapshot)
    return {"scope_count": len(snapshots), "scopes": snapshots}


def verify_terminal_isolation(workers: list[dict[str, Any]]) -> dict[str, Any]:
    markers = {
        pane["index"]: f"STAGE64_PANE_{pane['index']}_{uuid.uuid4().hex[:8]}"
        for pane in workers
    }
    for pane in workers:
        worker_call(
            pane["control_socket"],
            "SESSION_INPUT",
            {
                "session_id": pane["worker_session_id"],
                "data": f"printf '{markers[pane['index']]}\\n'\r",
            },
        )
    observed: dict[int, dict[str, Any]] = {}
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and len(observed) < len(workers):
        for pane in workers:
            if pane["index"] in observed:
                continue
            snapshot = worker_call(
                pane["control_socket"],
                "SESSION_ATTACH",
                {"session_id": pane["worker_session_id"]},
                timeout=2,
            )
            marker = markers[pane["index"]]
            journal = bytes.fromhex(snapshot["journal_hex"])
            if marker.encode() in snapshot["screen"].encode() and marker.encode() in journal:
                observed[pane["index"]] = snapshot
        if len(observed) < len(workers):
            time.sleep(0.05)
    if len(observed) != len(workers):
        missing = sorted(set(markers) - set(observed))
        raise SpikeFailure(f"terminal isolation markers missing for Panes {missing}")
    results = []
    for pane in workers:
        snapshot = observed[pane["index"]]
        screen = snapshot["screen"]
        journal = bytes.fromhex(snapshot["journal_hex"]).decode(errors="replace")
        for other, marker in markers.items():
            if other == pane["index"]:
                continue
            if marker in screen or marker in journal:
                raise SpikeFailure(
                    f"Pane {pane['index']} observed Pane {other} terminal state"
                )
        results.append(
            {
                "pane_index": pane["index"],
                "marker": markers[pane["index"]],
                "screen_contains_own_marker": True,
                "journal_contains_own_marker": True,
                "foreign_markers_absent": True,
                "next_stream_offset": snapshot["terminal"]["next_stream_offset"],
            }
        )
    return {"pane_count": len(results), "panes": results}


def stress_task_limit(
    pane: dict[str, Any], all_workers: list[dict[str, Any]]
) -> dict[str, Any]:
    worker_call(
        pane["control_socket"],
        "SESSION_INPUT",
        {
            "session_id": pane["worker_session_id"],
            "data": (
                "for i in 1 2 3 4 5 6 7 8 9 10 11 12; do sleep 30 & done; "
                "echo STAGE64_TASKS_SPAWNED; wait\r"
            ),
        },
    )
    observed = None
    deadline = time.monotonic() + 6
    while time.monotonic() < deadline:
        try:
            status = worker_call(
                pane["control_socket"], "WORKER_HELLO", timeout=1
            )
        except SpikeFailure:
            break
        metrics = status.get("resource_metrics", {})
        if metrics.get("last_violation") == "tasks_max":
            observed = status
            break
        time.sleep(0.05)
    if observed is None:
        raise SpikeFailure("task-limit pressure did not produce an enforcement event")
    if observed.get("resource_metrics", {}).get("last_violation") != "tasks_max":
        raise SpikeFailure(
            f"Pane exited without retaining tasks_max: {observed.get('resource_metrics')}"
        )
    enforcement = cgroup_snapshot(Path(pane["cgroup_path"]))
    if enforcement["pids_events"].get("max", 0) < 1:
        raise SpikeFailure("pids.events did not record a max hit")
    for other in all_workers:
        if other["pane_id"] != pane["pane_id"]:
            worker_call(other["control_socket"], "PING", timeout=2)
    return {
        "pane_index": pane["index"],
        "tasks_max": pane["policy"]["tasks_max"],
        "enforcement": observed["resource_metrics"]["last_violation"],
        "pane_state": observed.get("state"),
        "pids_events": enforcement["pids_events"],
        "other_panes_healthy": len(all_workers) - 1,
    }


def linux_process_identity(pid: int) -> dict[str, Any] | None:
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
        boot_id = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        fields = stat.rsplit(") ", 1)[1].split()
        return {
            "pid": pid,
            "pgid": int(fields[2]),
            "start_token": f"{boot_id}:{fields[19]}",
        }
    except (OSError, IndexError, ValueError):
        return None


def verify_crash_isolation(
    crashed: dict[str, Any], all_workers: list[dict[str, Any]]
) -> dict[str, Any]:
    os.kill(crashed["worker_pid"], signal.SIGKILL)
    wait_until(
        "crashed worker disappearance",
        lambda: not pid_alive(crashed["worker_pid"]),
        5,
    )
    healthy = []
    for other in all_workers:
        if other["pane_id"] != crashed["pane_id"]:
            worker_call(other["control_socket"], "PING", timeout=2)
            healthy.append(other["index"])
    try:
        worker_call(crashed["control_socket"], "PING", timeout=0.5)
        crashed_unreachable = False
    except (SpikeFailure, OSError):
        crashed_unreachable = True
    if not crashed_unreachable:
        raise SpikeFailure("crashed worker remained reachable")
    return {
        "crashed_pane_index": crashed["index"],
        "healthy_pane_indexes": healthy,
        "crashed_worker_unreachable": True,
    }


def recover_workers(manifest_path: Path) -> dict[str, Any]:
    manifest = read_json(manifest_path)
    workers = manifest["workers"]
    runtime_dir = Path(manifest["runtime_dir"])
    discovered = sorted(
        path
        for path in runtime_dir.iterdir()
        if path.is_socket() and path.suffix == ".sock"
    )
    expected = {Path(pane["control_socket"]) for pane in workers}
    if set(discovered) != expected:
        raise SpikeFailure(
            f"runtime discovery mismatch: found {discovered}, expected {expected}"
        )
    started = time.perf_counter()
    for pane in workers:
        status = worker_call(
            pane["control_socket"], "WORKER_HELLO", timeout=5
        )
        if status["worker_identity"] != pane["worker_identity"]:
            raise SpikeFailure(
                f"Pane {pane['index']} worker identity changed during adoption"
            )
        if status["process_identity"] != pane["task_identity"]:
            raise SpikeFailure(
                f"Pane {pane['index']} task identity changed during adoption"
            )
        worker_call(pane["control_socket"], "PING", timeout=2)
    recovery_ms = (time.perf_counter() - started) * 1000
    checks: dict[str, bool] = {
        "runtime_discovery": True,
        "worker_adoption": True,
        "health_ping": True,
    }
    failures: list[str] = []
    details: dict[str, Any] = {}

    def run_check(name: str, function: Callable[[], Any]) -> Any:
        try:
            value = function()
            checks[name] = True
            details[name] = value
            return value
        except BaseException as failure:
            checks[name] = False
            details[name] = {"error": str(failure)}
            failures.append(f"{name}: {failure}")
            return None

    cgroups = run_check("per_pane_cgroups", lambda: verify_cgroups(workers))
    run_check("per_pane_terminal_isolation", lambda: verify_terminal_isolation(workers))
    measurements = process_measurements(workers)
    details["process_measurements"] = measurements
    details["recovery_ms"] = recovery_ms

    target_pane = workers[0]
    run_check(
        "native_read_only_rejection",
        lambda: probe_read_only_rejection(
            target_pane["worker_session_id"], Path(target_pane["control_socket"])
        ),
    )
    time.sleep(0.2)
    run_check("targeted_attach", lambda: probe_exclusive_attach(target_pane))
    time.sleep(0.2)

    observer_pane = workers[min(2, len(workers) - 1)]
    observer_socket = Path(manifest["run_dir"]) / "observer.sock"
    forbidden_marker = Path(manifest["run_dir"]) / "observer-write-leak"
    observer = run_check(
        "read_only_observer",
        lambda: probe_observer_projection(
            observer_pane, observer_socket, forbidden_marker
        ),
    )
    if observer is not None:
        connection = sqlite3.connect(manifest["shared_store"])
        try:
            connection.execute(
                """
                INSERT INTO channel_grants
                    (id, principal_id, session_id, pane_id, profile, projection,
                     can_write, auth_context, created_at_ms, revoked_at_ms)
                VALUES (?, ?, ?, ?, 'human_observer', 'structured', 0, ?, ?, NULL)
                """,
                (
                    str(uuid.uuid4()),
                    LEGACY_PRINCIPAL_ID,
                    manifest["session_id"],
                    observer_pane["pane_id"],
                    json.dumps({"kind": "spike-opaque-context"}),
                    now_ms(),
                ),
            )
            connection.commit()
        finally:
            connection.close()

    run_check(
        "task_limit_enforcement_event",
        lambda: stress_task_limit(workers[-1], workers),
    )
    crash = run_check(
        "worker_crash_isolation",
        lambda: verify_crash_isolation(workers[1], workers),
    )
    if crash is not None:
        connection = sqlite3.connect(manifest["shared_store"])
        try:
            connection.execute(
                """
                UPDATE pane_workers SET
                    health = 'unreachable', lifecycle_state = 'lost',
                    updated_at_ms = ?
                WHERE pane_id = ?
                """,
                (now_ms(), workers[1]["pane_id"]),
            )
            connection.execute(
                """
                UPDATE tasks SET state = 'lost', ended_at_ms = ?
                WHERE pane_id = ? AND state = 'running'
                """,
                (now_ms(), workers[1]["pane_id"]),
            )
            connection.commit()
        finally:
            connection.close()
    manifest["crash"] = crash
    write_json(manifest_path, manifest)
    result = {
        "status": "pass" if all(checks.values()) else "fail",
        "checks": checks,
        "failures": failures,
        "discovered_sockets": [str(path) for path in discovered],
        "recovery_ms": recovery_ms,
        "process_measurements": measurements,
        "cgroups": cgroups,
        "details": details,
    }
    write_json(Path(manifest["run_dir"]) / "phases" / "recover.json", result)
    return result


def terminate_worker(pane: dict[str, Any], timeout: float = 3.0) -> dict[str, Any]:
    task_identity = pane["task_identity"]
    worker_identity = pane["worker_identity"]
    session_kill = "not-needed"
    if pid_alive(task_identity["pid"]):
        try:
            worker_call(
                pane["control_socket"],
                "SESSION_KILL",
                {"session_id": pane["worker_session_id"]},
                timeout=2,
            )
            session_kill = "worker"
        except SpikeFailure:
            session_kill = "fallback"
            if linux_process_identity(task_identity["pid"]) == task_identity:
                try:
                    os.killpg(task_identity["pgid"], signal.SIGKILL)
                except OSError:
                    pass
    scope_kill = "not-needed"
    cgroup = Path(pane["cgroup_path"])
    cgroup_kill = cgroup / "cgroup.kill"
    if cgroup_kill.exists():
        cgroup_kill.write_text("1")
        scope_kill = "cgroup.kill"
        deadline = time.monotonic() + timeout
        while cgroup.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
    if pid_alive(worker_identity["pid"]):
        try:
            os.kill(worker_identity["pid"], signal.SIGTERM)
        except OSError:
            pass
        deadline = time.monotonic() + timeout
        while pid_alive(worker_identity["pid"]) and time.monotonic() < deadline:
            time.sleep(0.05)
        if pid_alive(worker_identity["pid"]):
            try:
                os.kill(worker_identity["pid"], signal.SIGKILL)
            except OSError:
                pass
    wait_until(
        f"Pane {pane['index']} worker exit",
        lambda: not pid_alive(worker_identity["pid"]),
        5,
    )
    return {
        "pane_index": pane["index"],
        "session_kill": session_kill,
        "scope_kill": scope_kill,
        "worker_exited": True,
    }


def cleanup_workers(manifest_path: Path) -> dict[str, Any]:
    manifest = read_json(manifest_path)
    started = time.perf_counter()
    checks: dict[str, bool] = {}
    failures: list[str] = []
    terminated = []
    for pane in manifest["workers"]:
        try:
            terminated.append(terminate_worker(pane))
        except BaseException as failure:
            failures.append(f"Pane {pane['index']} termination: {failure}")
    cleanup_ms = (time.perf_counter() - started) * 1000
    residual_sockets = []
    for pane in manifest["workers"]:
        for path in (Path(pane["control_socket"]), Path(pane["attach_socket"])):
            try:
                path.unlink()
            except FileNotFoundError:
                pass
            except OSError as failure:
                failures.append(f"remove {path}: {failure}")
            if path.exists():
                residual_sockets.append(str(path))
    observer_socket = Path(manifest["run_dir"]) / "observer.sock"
    try:
        observer_socket.unlink()
    except FileNotFoundError:
        pass
    if observer_socket.exists():
        residual_sockets.append(str(observer_socket))
    checks["runtime_sockets_removed"] = not residual_sockets
    residual_cgroups = []
    for pane in manifest["workers"]:
        path = Path(pane["cgroup_path"])
        deadline = time.monotonic() + 10
        while path.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        if path.exists():
            residual_cgroups.append(str(path))
    checks["cgroup_scopes_removed"] = not residual_cgroups
    checks["worker_processes_exited"] = all(
        not pid_alive(pane["worker_pid"]) for pane in manifest["workers"]
    )
    connection = sqlite3.connect(manifest["shared_store"])
    try:
        now = now_ms()
        for pane in manifest["workers"]:
            connection.execute(
                """
                UPDATE pane_workers SET
                    health = 'absent', lifecycle_state = 'closed',
                    updated_at_ms = ?
                WHERE pane_id = ?
                """,
                (now, pane["pane_id"]),
            )
            connection.execute(
                """
                UPDATE tasks SET state = 'stopped', ended_at_ms = ?
                WHERE pane_id = ? AND state = 'running'
                """,
                (now, pane["pane_id"]),
            )
        connection.execute(
            """
            UPDATE resource_scopes SET
                lifecycle_state = 'closed', updated_at_ms = ?
            WHERE session_id = ? AND target_kind IN ('pane', 'task')
            """,
            (now, manifest["session_id"]),
        )
        connection.commit()
        remaining = connection.execute(
            "SELECT COUNT(*) FROM pane_workers WHERE health != 'absent'"
        ).fetchone()[0]
        checks["pane_worker_tombstones_persisted"] = remaining == 0
    finally:
        connection.close()
    temp_root = Path(manifest["temp_root"])
    if all(checks.values()) and not failures and temp_root.exists():
        shutil.rmtree(temp_root, ignore_errors=True)
    checks["temporary_runtime_removed"] = not temp_root.exists()
    result = {
        "status": "pass" if all(checks.values()) and not failures else "fail",
        "checks": checks,
        "failures": failures,
        "cleanup_ms": cleanup_ms,
        "terminated": terminated,
        "residual_sockets": residual_sockets,
        "residual_cgroups": residual_cgroups,
        "temp_root": str(temp_root),
    }
    write_json(Path(manifest["run_dir"]) / "phases" / "cleanup.json", result)
    return result


def validate_host() -> dict[str, Any]:
    if platform.system() != "Linux":
        raise SpikeFailure("Stage 6.4 resource verification requires Linux")
    cgroup_type = subprocess.check_output(
        ["stat", "-fc", "%T", "/sys/fs/cgroup"], text=True
    ).strip()
    if cgroup_type != "cgroup2fs":
        raise SpikeFailure(f"expected cgroup2fs, found {cgroup_type}")
    user_state = subprocess.run(
        ["systemctl", "--user", "is-system-running"],
        text=True,
        capture_output=True,
        check=False,
    ).stdout.strip()
    if user_state not in {"running", "degraded", "starting", "maintenance"}:
        raise SpikeFailure(f"systemd user manager is unavailable: {user_state}")
    controllers = set(read_text(Path("/sys/fs/cgroup/cgroup.controllers")).split())
    missing = sorted({"memory", "pids", "cpu"} - controllers)
    if missing:
        raise SpikeFailure(f"missing cgroup controllers: {missing}")
    return {
        "os": platform.platform(),
        "architecture": platform.machine(),
        "kernel": platform.release(),
        "cgroup_type": cgroup_type,
        "systemd_user_state": user_state,
        "controllers": sorted(controllers),
    }


def phase_command(args: argparse.Namespace, phase: str, run_dir: Path) -> list[str]:
    return [
        sys.executable,
        str(Path(__file__).resolve()),
        phase,
        "--run-dir",
        str(run_dir),
        "--worker",
        str(args.worker),
        "--qwed",
        str(args.qwed),
        "--panes",
        str(args.panes),
        "--repo-root",
        str(args.repo_root),
    ]


def run_phase_process(
    phase: str, args: argparse.Namespace, run_dir: Path
) -> dict[str, Any]:
    log_path = run_dir / f"{phase}.process.log"
    with log_path.open("wb") as log:
        completed = subprocess.run(
            phase_command(args, phase, run_dir),
            stdout=subprocess.PIPE,
            stderr=log,
            check=False,
        )
    if completed.returncode != 0:
        raise SpikeFailure(
            f"{phase} failed with exit {completed.returncode}:\n"
            f"{log_tail(log_path)}"
        )
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as failure:
        raise SpikeFailure(
            f"{phase} produced invalid JSON: {failure}\n"
            f"{completed.stdout.decode(errors='replace')}"
        ) from failure


def aggregate_run(
    run_dir: Path,
    environment: dict[str, Any],
    phase_results: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, Any]]:
    manifest = read_json(run_dir / "manifest.json")
    checks: dict[str, bool] = {}
    for result in phase_results.values():
        checks.update(result.get("checks", {}))
    status = "pass" if checks and all(checks.values()) else "fail"
    startups = phase_results.get("start", {}).get("startup_ms", {})
    recover = phase_results.get("recover", {})
    cleanup = phase_results.get("cleanup", {})
    measurements = recover.get("process_measurements", {})
    cgroups = recover.get("cgroups")
    metrics = {
        "pane_count": len(manifest.get("workers", [])),
        "startup_ms_max": max(startups.values(), default=0),
        "recovery_ms": recover.get("recovery_ms", 0),
        "cleanup_ms": cleanup.get("cleanup_ms", 0),
        "worker_processes": measurements.get("worker_processes", 0),
        "task_processes": measurements.get("task_processes", 0),
        "aggregate_rss_bytes": measurements.get("worker_rss_bytes", 0)
        + measurements.get("task_rss_bytes", 0),
        "resource_scopes": cgroups.get("scope_count", 0)
        if isinstance(cgroups, dict)
        else 0,
    }
    limitations = [
        "The spike-local supervisor groups distinct qwe-worker Session identities under one logical Session; production qwed does not yet expose this topology.",
        "Production V0 rejects read_only Attach; the observer path is validated through a spike-local qwed-owned bridge.",
        "The schema-v11 migration is a prototype and is not the accepted production migration.",
        "Only Fedora Linux x86_64 with systemd user cgroup v2 delegation was exercised.",
    ]
    report = {
        "stage": "6.4",
        "case": "SHARED-001",
        "status": status,
        "started_at": manifest["started_at"],
        "finished_at": utc_now(),
        "source_commit": manifest["source_commit"],
        "environment": environment,
        "manifest": manifest,
        "phase_results": phase_results,
        "checks": checks,
        "metrics": metrics,
        "known_limitations": limitations,
        "recommendation": (
            "During Stage 6.5, modify ADR 0021 to keep one worker per Pane and "
            "the ResourceScope interface, but define read-only observers as a "
            "qwed channel projection rather than a worker read_only mode."
            if status == "pass"
            else "Do not accept ADR 0021 until the failed checks are resolved."
        ),
    }
    started = datetime.fromisoformat(manifest["started_at"].replace("Z", "+00:00"))
    finished = datetime.fromisoformat(report["finished_at"].replace("Z", "+00:00"))
    result = {
        "case_id": "EX01",
        "language": "rust",
        "status": status,
        "source_commit": manifest["source_commit"],
        "environment": {
            "os": environment["os"],
            "architecture": environment["architecture"],
            "toolchain": f"rustc; python {platform.python_version()}",
            "kernel": environment["kernel"],
            "systemd_user_state": environment["systemd_user_state"],
        },
        "command": "just stage6-shared-prerequisites-spike",
        "started_at": manifest["started_at"],
        "duration_ms": max(0, (finished - started).total_seconds() * 1000),
        "metrics": metrics,
        "checks": checks,
        "evidence": [
            str(run_dir / "report.json"),
            str(run_dir / "phases" / "prepare.json"),
            str(run_dir / "phases" / "start.json"),
            str(run_dir / "phases" / "recover.json"),
            str(run_dir / "phases" / "cleanup.json"),
        ],
        "known_limitations": limitations,
    }
    return report, result


def write_summary(run_dir: Path, report: dict[str, Any]) -> None:
    checks = "\n".join(
        f"| `{name}` | {'PASS' if value else 'FAIL'} |"
        for name, value in sorted(report["checks"].items())
    )
    metrics = "\n".join(
        f"| `{name}` | {value} |" for name, value in report["metrics"].items()
    )
    text = f"""# Stage 6.4 Spike Summary

Status: **{report['status'].upper()}**

Source commit: `{report['source_commit']}`

## Checks

| Check | Result |
|---|---|
{checks}

## Measurements

| Metric | Value |
|---|---:|
{metrics}

## Recommendation

{report['recommendation']}

## Limitations

{chr(10).join(f"- {item}" for item in report['known_limitations'])}
"""
    (run_dir / "summary.md").write_text(text)


def run_all(args: argparse.Namespace) -> int:
    environment = validate_host()
    run_id = (
        datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        + "-"
        + uuid.uuid4().hex[:8]
    )
    run_dir = args.output / run_id
    run_dir.mkdir(parents=True, exist_ok=False)
    results: dict[str, Any] = {}
    try:
        for phase in ("prepare", "start"):
            results[phase] = run_phase_process(phase, args, run_dir)
            print(f"{phase}: {results[phase]['status']}", flush=True)
        results["recover"] = run_phase_process("recover", args, run_dir)
        print(f"recover: {results['recover']['status']}", flush=True)
    finally:
        try:
            results["cleanup"] = run_phase_process("cleanup", args, run_dir)
            print(f"cleanup: {results['cleanup']['status']}", flush=True)
        except BaseException as failure:
            results["cleanup"] = {
                "status": "fail",
                "checks": {"cleanup_completed": False},
                "error": str(failure),
            }
    report, result = aggregate_run(run_dir, environment, results)
    write_json(run_dir / "report.json", report)
    write_json(run_dir / "result.json", result)
    write_summary(run_dir, report)
    print(json.dumps(result, indent=2, sort_keys=True))
    print(f"evidence={run_dir}", flush=True)
    return 0 if report["status"] == "pass" else 1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="QWE Stage 6.4 spike")
    parser.add_argument(
        "phase",
        nargs="?",
        choices=("all", "prepare", "start", "recover", "cleanup"),
        default="all",
    )
    parser.add_argument("--run-dir", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--worker", type=Path)
    parser.add_argument("--qwed", type=Path)
    parser.add_argument("--panes", type=int, default=4)
    parser.add_argument("--repo-root", type=Path)
    args = parser.parse_args()
    args.repo_root = (args.repo_root or Path(__file__).resolve().parents[2]).resolve()
    args.output = (args.output or args.repo_root / "target" / "stage6.4").resolve()
    args.worker = (
        args.worker or args.repo_root / "target" / "debug" / "qwe-worker"
    ).resolve()
    args.qwed = (args.qwed or args.repo_root / "target" / "debug" / "qwed").resolve()
    if not 2 <= args.panes <= 4:
        parser.error("--panes must be between 2 and 4")
    if args.phase != "all" and args.run_dir is None:
        parser.error("--run-dir is required for a phase")
    if not args.worker.is_file():
        parser.error(f"worker binary does not exist: {args.worker}")
    if not args.qwed.is_file():
        parser.error(f"qwed binary does not exist: {args.qwed}")
    if not (args.worker.parent / "qwe-session-exec").is_file():
        parser.error("qwe-session-exec must be adjacent to qwe-worker")
    return args


def main() -> int:
    args = parse_args()
    try:
        validate_host()
        if args.phase == "all":
            return run_all(args)
        if args.phase == "prepare":
            result = prepare_run(
                args.run_dir,
                args.qwed,
                args.worker,
                args.panes,
                args.repo_root,
            )
        elif args.phase == "start":
            result = start_workers(args.run_dir / "manifest.json")
        elif args.phase == "recover":
            result = recover_workers(args.run_dir / "manifest.json")
        else:
            result = cleanup_workers(args.run_dir / "manifest.json")
        print(json.dumps(result, sort_keys=True))
        return 0
    except BaseException as failure:
        print(f"stage6.4 spike failed: {failure}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
