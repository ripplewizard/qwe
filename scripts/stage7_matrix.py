#!/usr/bin/env python3

import argparse
import json
import shlex
import subprocess
from pathlib import Path


EXPECTED = {
    "fedora-x86_64",
    "ubuntu-22.04-x86_64",
    "ubuntu-24.04-x86_64",
    "debian-12-x86_64",
}


def run_target(target, repo, soak_seconds):
    command = (
        f"cd {shlex.quote(repo)} && "
        "python3 tests/fixtures/stage7_node_gate.py "
        f"--soak-seconds {soak_seconds}"
    )
    if target == "local":
        shell = ["bash", "-lc", command]
    else:
        shell = [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            target,
            command,
        ]
    completed = subprocess.run(
        shell,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=max(3600, int(soak_seconds + 1800)),
    )
    if completed.returncode != 0 and "No route to host" in completed.stderr:
        return {"status": "blocked", "error": completed.stderr.strip()}
    lines = [line for line in completed.stdout.splitlines() if line.startswith("{")]
    if not lines:
        status = "blocked" if completed.returncode == 255 else "fail"
        return {
            "status": status,
            "error": (completed.stderr or completed.stdout)[-4000:],
        }
    result = json.loads(lines[-1])
    if completed.returncode != 0 and result.get("status") == "pass":
        result["status"] = "fail"
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--soak-seconds", type=float, default=60)
    parser.add_argument(
        "--target",
        action="append",
        default=[],
        help="NAME=SSH_TARGET, or NAME=local",
    )
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if not args.target:
        args.target = [
            "fedora-x86_64=local",
            "ubuntu-22.04-x86_64=UNAVAILABLE",
            "ubuntu-24.04-x86_64=UNAVAILABLE",
            "debian-12-x86_64=UNAVAILABLE",
        ]

    results = {}
    for entry in args.target:
        if "=" not in entry:
            parser.error(f"target must be NAME=SSH_TARGET: {entry}")
        name, target = entry.split("=", 1)
        if target == "UNAVAILABLE":
            results[name] = {
                "status": "blocked",
                "error": "no configured real-VM SSH target",
            }
            continue
        results[name] = run_target(target, str(args.repo), args.soak_seconds)

    missing = sorted(EXPECTED - results.keys())
    statuses = [result.get("status") for result in results.values()]
    if missing or "blocked" in statuses:
        overall = "blocked"
    elif "fail" in statuses:
        overall = "fail"
    else:
        overall = "pass"
    manifest = {
        "case": "RG-005",
        "expected_targets": sorted(EXPECTED),
        "missing_targets": missing,
        "status": overall,
        "targets": results,
    }
    rendered = json.dumps(manifest, sort_keys=True, separators=(",", ":"))
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered + "\n")
    print(rendered)
    if manifest["status"] != "pass":
        raise SystemExit(2)


if __name__ == "__main__":
    main()
