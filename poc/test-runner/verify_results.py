#!/usr/bin/env python3
"""Validate the latest structured result and referenced evidence per case."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

CASE = re.compile(r"^(?:T|P|D|RM|DX|XP|EX|MC)[0-9]{2}$")


def validate_schema(value, schema, path="$"):
    errors = []
    expected_type = schema.get("type")
    if expected_type is not None:
        expected = expected_type if isinstance(expected_type, list) else [expected_type]
        matches = False
        for type_name in expected:
            if type_name == "object" and isinstance(value, dict):
                matches = True
            elif type_name == "array" and isinstance(value, list):
                matches = True
            elif type_name == "string" and isinstance(value, str):
                matches = True
            elif type_name == "boolean" and isinstance(value, bool):
                matches = True
            elif type_name == "null" and value is None:
                matches = True
            elif (
                type_name == "number"
                and isinstance(value, (int, float))
                and not isinstance(value, bool)
            ):
                matches = True
        if not matches:
            errors.append(f"{path}: expected type {expected_type}")
            return errors

    if "enum" in schema and value not in schema["enum"]:
        errors.append(f"{path}: value is not in enum")
    if "pattern" in schema and isinstance(value, str):
        if re.fullmatch(schema["pattern"], value) is None:
            errors.append(f"{path}: value does not match pattern")
    if "minLength" in schema and isinstance(value, str):
        if len(value) < schema["minLength"]:
            errors.append(f"{path}: string is shorter than minLength")
    if "minItems" in schema and isinstance(value, list):
        if len(value) < schema["minItems"]:
            errors.append(f"{path}: array is shorter than minItems")
    if "anyOf" in schema:
        alternatives = [
            validate_schema(value, option, path) for option in schema["anyOf"]
        ]
        if all(alternative for alternative in alternatives):
            errors.append(f"{path}: value does not match any allowed schema")

    if isinstance(value, dict):
        for key in schema.get("required", []):
            if key not in value:
                errors.append(f"{path}: missing required property {key}")
        for key, child_schema in schema.get("properties", {}).items():
            if key in value:
                errors.extend(validate_schema(value[key], child_schema, f"{path}.{key}"))
        if schema.get("additionalProperties") is False:
            allowed = set(schema.get("properties", {}))
            for key in value.keys() - allowed:
                errors.append(f"{path}: unexpected property {key}")
        child_schema = schema.get("additionalProperties")
        if isinstance(child_schema, dict):
            for key, child in value.items():
                if key not in schema.get("properties", {}):
                    errors.extend(validate_schema(child, child_schema, f"{path}.{key}"))

    if isinstance(value, list) and isinstance(schema.get("items"), dict):
        for index, child in enumerate(value):
            errors.extend(validate_schema(child, schema["items"], f"{path}[{index}]"))
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results-root", required=True, type=Path)
    parser.add_argument("--schema", required=True, type=Path)
    parser.add_argument("--matrix", required=True, type=Path)
    parser.add_argument(
        "--platform",
        action="append",
        choices=("linux-x86_64", "macos-arm64"),
        help="verify only this platform; repeatable",
    )
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

    platforms = tuple(args.platform or ("linux-x86_64", "macos-arm64"))
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
                schema_errors = validate_schema(result, schema)
                if schema_errors:
                    errors.append(f"{path}: schema: {schema_errors[0]}")
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
