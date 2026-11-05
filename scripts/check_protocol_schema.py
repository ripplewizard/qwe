#!/usr/bin/env python3
"""Validate the canonical QWE contract and its checked-in drift surfaces."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

try:
    from jsonschema import Draft202012Validator
    from jsonschema.exceptions import SchemaError, ValidationError
except ImportError as failure:
    raise SystemExit(
        "protocol-check requires Python package 'jsonschema' "
        "(install scripts/requirements-protocol.txt)"
    ) from failure


ROOT = Path(__file__).resolve().parents[1]
SCHEMA_PATH = ROOT / "protocol/schema/qwe-v1.schema.json"
MATRIX_PATH = ROOT / "docs/test-matrices.md"
PROTOCOL_SOURCE = ROOT / "crates/qwe-protocol/src/lib.rs"
TEST_ID_PATTERN = re.compile(r"\b(?:AUTH|LIVE|ATTACH|OBJ|IDEMP|HIST)-\d{3}\b")


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as failure:
        raise SystemExit(f"{path}: {failure}") from failure
    if not isinstance(value, dict):
        raise SystemExit(f"{path}: schema root must be an object")
    return value


def definition_validator(schema: dict[str, Any], name: str) -> Draft202012Validator:
    definitions = schema["$defs"]
    subschema = {
        "$schema": schema["$schema"],
        "$id": schema["$id"],
        "$defs": definitions,
        **definitions[name],
    }
    return Draft202012Validator(subschema)


def validate_definition(
    schema: dict[str, Any], name: str, value: dict[str, Any]
) -> None:
    errors = sorted(
        definition_validator(schema, name).iter_errors(value),
        key=lambda error: list(error.absolute_path),
    )
    if errors:
        message = "; ".join(error.message for error in errors)
        raise SystemExit(f"{name} example is invalid: {message}")


def reject_definition(
    schema: dict[str, Any], name: str, value: dict[str, Any], label: str
) -> None:
    if definition_validator(schema, name).is_valid(value):
        raise SystemExit(f"{label} must be rejected by {name}")


def parse_frame_types() -> dict[str, int]:
    source = PROTOCOL_SOURCE.read_text(encoding="utf-8")
    match = re.search(
        r"pub enum FrameType \{(?P<body>.*?)\n    \}",
        source,
        flags=re.DOTALL,
    )
    if match is None:
        raise SystemExit("could not locate qwe-protocol FrameType")
    entries = re.findall(
        r"^\s*(?P<name>[A-Za-z][A-Za-z0-9]*)\s*=\s*(?P<code>0x[0-9A-Fa-f]+),$",
        match.group("body"),
        flags=re.MULTILINE,
    )
    if not entries:
        raise SystemExit("could not parse qwe-protocol FrameType entries")
    return {
        re.sub(r"(?<!^)(?=[A-Z])", "_", name).upper(): int(code, 16)
        for name, code in entries
    }


def rust_usize_constant(name: str) -> int:
    source = PROTOCOL_SOURCE.read_text(encoding="utf-8")
    match = re.search(
        rf"pub const {name}: usize = (?P<value>[0-9_\s*]+);",
        source,
    )
    if match is None:
        raise SystemExit(f"could not locate qwe-protocol constant {name}")
    expression = match.group("value").replace("_", "").replace(" ", "")
    if not re.fullmatch(r"[0-9*]+", expression):
        raise SystemExit(f"unsupported expression for {name}: {expression}")
    return int(eval(expression, {"__builtins__": {}}, {}))


def validate_examples(schema: dict[str, Any]) -> None:
    validate_definition(
        schema,
        "AttachOpen",
        {
            "protocol_version": 1,
            "session_id": "session-1",
            "owner": "client-a",
            "rows": 24,
            "columns": 80,
            "mode": "exclusive",
        },
    )
    validate_definition(
        schema, "Resize", {"protocol_version": 1, "rows": 40, "columns": 120}
    )
    validate_definition(
        schema, "Ack", {"protocol_version": 1, "stream_offset": 4096}
    )
    validate_definition(
        schema, "Detach", {"protocol_version": 1, "hold_ttl_ms": 0}
    )
    validate_definition(
        schema,
        "AttachAccepted",
        {
            "protocol_version": 1,
            "session_id": "session-1",
            "owner": "client-a",
            "generation": 3,
            "reclaim_token": "opaque-token",
            "rows": 24,
            "columns": 80,
            "stream_offset": 4096,
        },
    )
    validate_definition(
        schema,
        "TerminalSnapshot",
        {
            "rows": 24,
            "columns": 80,
            "cursor_row": 3,
            "cursor_column": 12,
            "cursor_visible": True,
            "cursor_style": "steady_block",
            "alternate_screen": False,
            "application_cursor": True,
            "application_keypad": False,
            "bracketed_paste": True,
            "mouse_protocol_mode": "button_motion",
            "mouse_protocol_encoding": "sgr",
            "screen_text": "QWE",
            "next_stream_offset": 4096,
        },
    )
    validate_definition(
        schema,
        "InputMetadata",
        {"frame_type": "INPUT", "payload_encoding": "raw"},
    )
    validate_definition(
        schema,
        "OutputMetadata",
        {
            "frame_type": "OUTPUT",
            "offset_bytes": 8,
            "offset_encoding": "u64_be",
            "payload_encoding": "raw",
            "maximum_output_bytes": 65536,
        },
    )
    validate_definition(
        schema,
        "Resized",
        {
            "protocol_version": 1,
            "rows": 40,
            "columns": 120,
            "stream_offset": 8192,
        },
    )
    validate_definition(
        schema,
        "ProcessExited",
        {
            "protocol_version": 1,
            "exit_code": None,
            "exit_signal": "SIGTERM",
            "stream_offset": 8192,
        },
    )
    validate_definition(
        schema,
        "OwnershipRevoked",
        {"protocol_version": 1, "reason": "supervisor_restart"},
    )
    validate_definition(
        schema,
        "AttachError",
        {
            "protocol_version": 1,
            "code": "output_backpressure",
            "message": "client output queue overflowed",
        },
    )
    reject_definition(
        schema,
        "AttachOpen",
        {
            "protocol_version": 1,
            "session_id": "session-1",
            "owner": "client-a",
            "rows": 0,
            "columns": 80,
            "mode": "exclusive",
        },
        "zero terminal rows",
    )
    reject_definition(
        schema,
        "OutputMetadata",
        {
            "frame_type": "OUTPUT",
            "offset_bytes": 8,
            "offset_encoding": "u64_be",
            "payload_encoding": "base64",
            "maximum_output_bytes": 65536,
        },
        "encoded OUTPUT payload",
    )


def validate_drift(schema: dict[str, Any]) -> None:
    try:
        Draft202012Validator.check_schema(schema)
    except SchemaError as failure:
        raise SystemExit(f"invalid JSON Schema: {failure.message}") from failure

    schema_ids = schema["x-qwe-test-case-ids"]
    if len(schema_ids) != len(set(schema_ids)):
        raise SystemExit("canonical schema contains duplicate test-case IDs")

    occurrences = TEST_ID_PATTERN.findall(MATRIX_PATH.read_text(encoding="utf-8"))
    matrix_ids = set(occurrences)
    if len(occurrences) != len(matrix_ids):
        raise SystemExit("test matrix contains duplicate test-case IDs")
    if set(schema_ids) != matrix_ids:
        missing_from_schema = sorted(matrix_ids - set(schema_ids))
        missing_from_matrix = sorted(set(schema_ids) - matrix_ids)
        raise SystemExit(
            "test-case ID drift: "
            f"missing_from_schema={missing_from_schema}, "
            f"missing_from_matrix={missing_from_matrix}"
        )

    rust_frames = parse_frame_types()
    schema_frames = {
        **schema["x-qwe-frame-types"]["client_to_server"],
        **schema["x-qwe-frame-types"]["server_to_client"],
    }
    if rust_frames != schema_frames:
        raise SystemExit(
            "attach frame drift: "
            f"rust_only={sorted(rust_frames.keys() - schema_frames.keys())}, "
            f"schema_only={sorted(schema_frames.keys() - rust_frames.keys())}, "
            f"different_codes="
            f"{sorted(name for name in rust_frames.keys() & schema_frames.keys() if rust_frames[name] != schema_frames[name])}"
        )

    header = schema["x-qwe-frame-header"]
    if header["maximum_payload_bytes"] != rust_usize_constant(
        "MAX_ATTACH_FRAME_PAYLOAD_BYTES"
    ):
        raise SystemExit("attach frame payload limit differs between schema and Rust")
    if schema["$defs"]["OutputMetadata"]["properties"]["maximum_output_bytes"][
        "const"
    ] != rust_usize_constant("MAX_ATTACH_OUTPUT_BYTES"):
        raise SystemExit("attach OUTPUT limit differs between schema and Rust")

    validate_examples(schema)


def main() -> int:
    schema = load_json(SCHEMA_PATH)
    try:
        validate_drift(schema)
    except ValidationError as failure:
        raise SystemExit(f"protocol example validation failed: {failure.message}") from failure
    print(
        "protocol schema PASS "
        f"({len(schema['x-qwe-frame-types']['client_to_server']) + len(schema['x-qwe-frame-types']['server_to_client'])} frame types, "
        f"{len(schema['x-qwe-test-case-ids'])} stable test IDs)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
