#!/usr/bin/env python3
"""Validate the latest structured result and referenced evidence per case."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

import jsonschema


CASE = re.compile(r"^(?:T|P|D|RM|DX|XP|EX|MC)[0-9]{2}$")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results-root", required=True, type=Path)
    parser.add_argument("--schema", required=True, type=Path)
    parser.add_argument("--matrix", required=True, type=Path)
    args = parser.parse_args()
    repo_root = args.results_root.parent.parent
    schema = json.loads(args.schema.read_text(encoding="utf-8"))
    matrix_text = args.matrix.read_text(encoding="utf-8")
    expected = set(
        re.findall(
            r"^\| ((?:T|P|D|RM|DX|XP|EX|MC)[0-9]{2}) \|",
            matrix_text,
            re.MULTILINE,
        )
    )
    latest = {}
    errors = []
    for path in args.results_root.rglob("*.json"):
        if path.name.endswith(".stdout.json"):
            continue
        try:
            result = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            errors.append(f"{path}: invalid JSON: {error}")
            continue
        if not isinstance(result, dict):
            continue
        case_id = result.get("case_id")
        language = result.get("language")
        if not CASE.fullmatch(str(case_id)) or language not in {"rust", "go", "cpp"}:
            continue
        platform = path.parents[1].name
        if not platform.startswith(("linux-", "macos-")):
            continue
        key = (platform, language, case_id)
        if key not in latest or result.get("started_at", "") > latest[key].get(
            "started_at", ""
        ):
            latest[key] = result | {"_path": path}

    platforms = ("linux-x86_64", "macos-arm64")
    languages = ("rust", "go", "cpp")
    for platform in platforms:
        for language in languages:
            for case_id in sorted(expected):
                key = (platform, language, case_id)
                result = latest.get(key)
                if result is None:
                    errors.append(f"missing latest result: {platform}/{language}/{case_id}")
                    continue
                path = result.pop("_path")
                try:
                    jsonschema.validate(result, schema)
                except jsonschema.ValidationError as error:
                    errors.append(f"{path}: schema: {error.message}")
                    continue
                for evidence in result.get("evidence", []):
                    if evidence.startswith("poc/test-runner/"):
                        continue
                    if not (repo_root / evidence).exists():
                        errors.append(f"{path}: missing evidence: {evidence}")
                environment_record = result.get("environment", {}).get(
                    "environment_record"
                )
                if environment_record and not (repo_root / environment_record).exists():
                    errors.append(
                        f"{path}: missing environment record: {environment_record}"
                    )

    if errors:
        for error in errors:
            print(error)
        print(f"result verification FAIL ({len(errors)} errors)")
        return 1
    print(
        f"result verification PASS ({len(latest)} latest results, "
        f"{len(expected) * len(platforms) * len(languages)} expected)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
