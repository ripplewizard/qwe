#!/usr/bin/env python3
"""Refresh the latest-evidence section of the POC matrix."""

from __future__ import annotations

import argparse
import json
import re
import time
from pathlib import Path


PLATFORMS = ("linux-x86_64", "macos-arm64")
LANGUAGES = ("rust", "go", "cpp")
CASE = re.compile(r"^(?:T|P|D|RM|DX|XP|EX|MC)[0-9]{2}$")
MARKER = "<!-- LATEST_EVIDENCE_START -->"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results-root", required=True, type=Path)
    parser.add_argument("--matrix", required=True, type=Path)
    args = parser.parse_args()

    original = args.matrix.read_text(encoding="utf-8")
    prefix = original.split(MARKER, 1)[0].rstrip() + "\n\n" + MARKER + "\n\n"
    cases = sorted(
        set(
            re.findall(
                r"^\| ((?:T|P|D|RM|DX|XP|EX|MC)[0-9]{2}) \|",
                prefix,
                re.MULTILINE,
            )
        )
    )
    latest = {}
    for platform in PLATFORMS:
        for language in LANGUAGES:
            for path in (args.results_root / platform / language).glob("*.json"):
                try:
                    result = json.loads(path.read_text(encoding="utf-8"))
                except (OSError, json.JSONDecodeError):
                    continue
                case_id = result.get("case_id")
                if not CASE.fullmatch(str(case_id)):
                    continue
                key = (platform, language, case_id)
                previous = latest.get(key)
                if previous is None or result.get("started_at", "") > previous.get(
                    "started_at", ""
                ):
                    result["_path"] = str(path.relative_to(args.results_root.parent.parent))
                    latest[key] = result

    generated = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    lines = [f"Generated: {generated}", "", "| ID | Linux R/G/C | macOS R/G/C | Evidence |", "|---|---:|---:|---|"]
    for case_id in cases:
        linux = [latest.get(("linux-x86_64", language, case_id)) for language in LANGUAGES]
        macos = [latest.get(("macos-arm64", language, case_id)) for language in LANGUAGES]
        status = lambda result: result.get("status", "missing") if result else "missing"
        representative = next((result for result in linux + macos if result), None)
        evidence = representative["_path"] if representative else "missing"
        lines.append(
            f"| {case_id} | "
            + "/".join(status(result) for result in linux)
            + " | "
            + "/".join(status(result) for result in macos)
            + f" | `{evidence}` |"
        )
    args.matrix.write_text(prefix + "\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps({"cases": len(cases), "generated": generated}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
