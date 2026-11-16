#!/usr/bin/env python3

import argparse
import json
import os
import platform
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures"


def run_probe(name, arguments, timeout=1800):
    completed = subprocess.run(
        [sys.executable, str(FIXTURES / name), *arguments],
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
    )
    lines = [line for line in completed.stdout.splitlines() if line.startswith("{")]
    if completed.returncode != 0 or not lines:
        raise RuntimeError(
            f"{name} failed with {completed.returncode}: "
            f"{completed.stdout[-2000:]} {completed.stderr[-2000:]}"
        )
    return json.loads(lines[-1])


def system_info():
    values = {}
    for line in Path("/etc/os-release").read_text().splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            values[key] = value.strip().strip('"')
    controllers = set(
        Path("/sys/fs/cgroup/cgroup.controllers").read_text().split()
    )
    return {
        "distribution": values.get("ID", "unknown"),
        "distribution_version": values.get("VERSION_ID", "unknown"),
        "kernel": platform.release(),
        "architecture": platform.machine(),
        "systemd_user": subprocess.run(
            ["systemctl", "--user", "is-system-running"],
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        ).stdout.strip(),
        "cgroup_v2": Path("/sys/fs/cgroup/cgroup.controllers").is_file(),
        "controllers": {
            "cpu": "cpu" in controllers,
            "memory": "memory" in controllers,
            "pids": "pids" in controllers,
        },
    }


def supported_distribution(info):
    return (
        info["distribution"] == "fedora"
        or (
            info["distribution"] == "ubuntu"
            and info["distribution_version"] in {"22.04", "24.04"}
        )
        or (
            info["distribution"] == "debian"
            and info["distribution_version"] == "12"
        )
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--soak-seconds", type=float, default=60)
    args = parser.parse_args()
    info = system_info()
    result = {
        "case": "RG-005",
        **info,
        "status": "running",
        "checks": {},
    }
    if not supported_distribution(info):
        result["status"] = "unsupported"
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        raise SystemExit(2)
    if (
        info["systemd_user"] not in {"running", "degraded", "starting", "maintenance"}
        or not info["cgroup_v2"]
        or not all(info["controllers"].values())
    ):
        result["status"] = "blocked"
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        raise SystemExit(2)

    build = subprocess.run(
        ["cargo", "build", "--release", "-p", "qwe", "-p", "qwed", "-p", "qwe-worker"],
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=1800,
    )
    result["checks"]["release_build"] = build.returncode == 0
    if build.returncode != 0:
        result["status"] = "fail"
        result["error"] = f"release build failed: {build.stderr[-2000:]}"
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        raise SystemExit(1)

    qwed = ROOT / "target/release/qwed"
    qwe = ROOT / "target/release/qwe"
    try:
        with tempfile.TemporaryDirectory(prefix="qwe-stage7-node-") as root:
            root = Path(root)
            result["profiles"] = run_probe(
                "stage7_resource_probe.py",
                [
                    "--qwed",
                    str(qwed),
                    "--qwe",
                    str(qwe),
                    "--socket",
                    str(root / "profiles.sock"),
                ],
                timeout=180,
            )
            result["checks"]["profiles"] = True
            result["multi_pane"] = run_probe(
                "stage7_multi_pane_probe.py",
                [
                    "--qwed",
                    str(qwed),
                    "--socket",
                    str(root / "multi.sock"),
                ],
                timeout=180,
            )
            result["checks"]["multi_pane"] = True
            result["pressure"] = run_probe(
                "stage7_pressure_probe.py",
                [
                    "--qwed",
                    str(qwed),
                    "--socket",
                    str(root / "pressure.sock"),
                ],
                timeout=180,
            )
            result["checks"]["pressure"] = True
            result["soak"] = run_probe(
                "stage7_soak_probe.py",
                [
                    "--qwed",
                    str(qwed),
                    "--sessions",
                    "8",
                    "--panes-per-session",
                    "2",
                    "--duration-seconds",
                    str(args.soak_seconds),
                    "--restart-interval-seconds",
                    str(max(5, args.soak_seconds / 4)),
                ],
                timeout=max(1800, int(args.soak_seconds + 300)),
            )
            result["checks"]["short_soak"] = True
            result["service_lifecycle"] = run_probe(
                "service_lifecycle_probe.py",
                [
                    "--qwed",
                    str(qwed),
                    "--qwe",
                    str(qwe),
                    "--socket",
                    str(root / "service.sock"),
                ],
                timeout=180,
            )
            result["checks"]["installed_systemd_service"] = True

        residual = subprocess.run(
            [
                "systemctl",
                "--user",
                "list-units",
                "qwe-pane-*.scope",
                "--all",
                "--plain",
                "--no-legend",
            ],
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        residual_scopes = [
            line.split()[0] for line in residual.stdout.splitlines() if line.strip()
        ]
        result["residual_scopes"] = residual_scopes
        result["checks"]["scope_cleanup"] = not residual_scopes
        result["status"] = (
            "pass" if all(result["checks"].values()) else "fail"
        )
    except Exception as failure:
        result["status"] = "fail"
        result["error"] = str(failure)

    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    if result["status"] != "pass":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
