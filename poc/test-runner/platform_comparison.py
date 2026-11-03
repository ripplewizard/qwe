#!/usr/bin/env python3
"""Generate XP01 cross-platform coverage and performance evidence."""

from __future__ import annotations

import argparse
import json
import os
import re
import time
from collections import Counter
from pathlib import Path
from typing import Any


LANGUAGES = ("rust", "go", "cpp")
PLATFORMS = ("linux-x86_64", "macos-arm64")


def latest_results(root: Path) -> dict[tuple[str, str, str], dict[str, Any]]:
    latest: dict[tuple[str, str, str], dict[str, Any]] = {}
    for platform in PLATFORMS:
        for language in LANGUAGES:
            for path in (root / platform / language).glob("*.json"):
                try:
                    result = json.loads(path.read_text(encoding="utf-8"))
                except (OSError, json.JSONDecodeError):
                    continue
                case_id = result.get("case_id")
                if not isinstance(case_id, str):
                    continue
                key = (platform, language, case_id)
                previous = latest.get(key)
                if previous is None or result.get("started_at", "") > previous.get(
                    "started_at", ""
                ):
                    result["_path"] = str(path)
                    latest[key] = result
    return latest


def expected_cases(matrix: Path) -> list[str]:
    text = matrix.read_text(encoding="utf-8")
    case_ids = re.findall(
        r"^\| ((?:T|P|D|RM|DX|XP|EX|MC)[0-9]{2}) \|",
        text,
        re.MULTILINE,
    )
    return list(dict.fromkeys(case_id for case_id in case_ids if case_id != "XP01"))


def fmt(value: Any) -> str:
    if isinstance(value, float):
        return f"{value:.3f}"
    return str(value)


def metric_value(
    results: dict[tuple[str, str, str], dict[str, Any]],
    platform: str,
    language: str,
    case_id: str,
    *path: str,
) -> Any:
    value: Any = results.get((platform, language, case_id), {}).get("metrics", {})
    for key in path:
        if not isinstance(value, dict) or key not in value:
            return None
        value = value[key]
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results-root", required=True, type=Path)
    parser.add_argument("--matrix", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    repo_root = args.results_root.parent.parent
    results_root = args.results_root
    results = latest_results(args.results_root)
    cases = expected_cases(args.matrix)
    generated_at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    commits = Counter(
        result.get("source_commit", "unknown") for result in results.values()
    )
    source_commit = commits.most_common(1)[0][0] if commits else "unknown"

    missing: dict[tuple[str, str], list[str]] = {}
    for platform in PLATFORMS:
        for language in LANGUAGES:
            absent = [
                case_id
                for case_id in cases
                if (platform, language, case_id) not in results
            ]
            if absent:
                missing[(platform, language)] = absent

    lines = [
        "# XP01 Platform Capability and Performance Comparison",
        "",
        "Status: generated from latest structured result per case and language.",
        "",
        "## Metadata",
        "",
        f"- Generated: {generated_at}",
        f"- Source commit: `{source_commit}`",
        "- Primary target: Linux x86_64 (distribution version is metadata).",
        "- Secondary target: macOS ARM64 preflight.",
        "- Selection: latest result by `started_at` for each platform/language/case.",
        "",
        "## Coverage Matrix",
        "",
        "| Case | Linux Rust | Linux Go | Linux C++ | macOS Rust | macOS Go | macOS C++ |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for case_id in cases:
        cells = []
        for platform in PLATFORMS:
            for language in LANGUAGES:
                result = results.get((platform, language, case_id))
                cells.append(result.get("status", "missing") if result else "missing")
        lines.append(f"| {case_id} | " + " | ".join(cells) + " |")

    lines += [
        "",
        "## Performance Snapshot",
        "",
        "| Metric | Linux Rust | Linux Go | Linux C++ | macOS Rust | macOS Go | macOS C++ |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    metrics = [
        ("CLI probe median (ms)", "DX01", ("cli_startup_median_ms",)),
        ("IPC status median (ms)", "DX01", ("ipc_missing_status_median_ms",)),
        ("Clean build (ms)", "DX03", ("clean_build_ms",)),
        ("50k-line output (ms)", "T10", ("elapsed_ms",)),
        (
            "Daemon RSS (KiB)",
            "D08",
            ("rss_after_kib",),
        ),
        (
            "100 ms sample latency (ms)",
            "RM03",
            ("sample_latency_100_ms",),
        ),
        (
            "1000 ms sample latency (ms)",
            "RM03",
            ("sample_latency_1000_ms",),
        ),
        (
            "5000 ms sample latency (ms)",
            "RM03",
            ("sample_latency_5000_ms",),
        ),
    ]
    for label, case_id, path in metrics:
        row = [
            metric_value(results, platform, language, case_id, *path)
            for platform in PLATFORMS
            for language in LANGUAGES
        ]
        lines.append(f"| {label} | " + " | ".join(fmt(v) for v in row) + " |")

    gpu_values = []
    for platform in PLATFORMS:
        for language in LANGUAGES:
            gpu_values.append(
                metric_value(results, platform, language, "RM04", "gpu_supported")
            )
    lines += [
        "",
        "## Capability Differences",
        "",
        f"- GPU capability flags: {gpu_values}.",
        "- Linux exposes task-attributable GPU capability on the current host; macOS reports `unsupported` without a trustworthy process mapping.",
        "- Linux restart identity uses `/proc` process start time; the current macOS token has one-second resolution.",
        "- T12 currently verifies a deliberate boundary: daemon restart does not restore terminal process, PTY, screen, metadata, or input.",
        "- Manual T03/T04/T08/T11/T12 review records are platform evidence, not language-specific runtime scores.",
        "",
        "## Missing Evidence",
        "",
    ]
    if missing:
        for (platform, language), absent in sorted(missing.items()):
            lines.append(f"- `{platform}/{language}`: {', '.join(absent)}")
    else:
        lines.append("- No matrix case is missing from Rust, Go, or C++ on either platform.")

    lines += [
        "",
        "## Interpretation Limits",
        "",
        "- Results come from dirty worktrees until the accepted implementation is committed and regenerated.",
        "- Single-host measurements are feasibility data, not production benchmarks.",
        "- `blocked` means manual review or a deliberately unclaimed recovery capability remains; it is not a pass.",
        "- Distribution version is recorded as environment metadata, not a target restriction.",
        "",
    ]
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text("\n".join(lines), encoding="utf-8")

    for platform in PLATFORMS:
        for language in LANGUAGES:
            absent = missing.get((platform, language), [])
            checks = {
                "linux_primary_present": any(
                    pid == "linux-x86_64" and lid == language
                    for pid, lid, _ in results
                ),
                "macos_secondary_present": any(
                    pid == "macos-arm64" and lid == language
                    for pid, lid, _ in results
                ),
                "matrix_coverage_complete": not absent,
                "manual_review_records_present": all(
                    (
                        results_root / current_platform / "manual" / f"{case_id}.md"
                    ).exists()
                    for current_platform in PLATFORMS
                    for case_id in ("T03", "T04", "T08", "T11", "T12")
                ),
                "t12_recovery_not_overclaimed": all(
                    results.get((pid, language, "T12"), {}).get("status") != "pass"
                    for pid in PLATFORMS
                ),
            }
            result = {
                "case_id": "XP01",
                "language": language,
                "status": "pass" if all(checks.values()) else "fail",
                "source_commit": source_commit,
                "environment": {
                    "os": "cross-platform",
                    "architecture": "linux-x86_64+macos-arm64",
                    "toolchain": "per-language result metadata",
                },
                "command": "python3 poc/test-runner/platform_comparison.py",
                "started_at": generated_at,
                "duration_ms": 0,
                "metrics": {
                    "linux_cases": sum(
                        1 for pid, lid, _ in results if pid == "linux-x86_64" and lid == language
                    ),
                    "macos_cases": sum(
                        1 for pid, lid, _ in results if pid == "macos-arm64" and lid == language
                    ),
                    "missing_case_count": len(absent),
                },
                "checks": checks,
                "evidence": [str(args.report.relative_to(repo_root))],
                "known_limitations": [
                    "Comparison reflects latest per-case files rather than a clean simultaneous rerun.",
                    "Dirty source evidence remains provisional.",
                ],
            }
            destination = (
                args.results_root
                / platform
                / language
                / f"XP01-{source_commit}-{os.getpid()}.json"
            )
            destination.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")

    print(
        json.dumps(
            {
                "report": str(args.report),
                "cases": len(cases),
                "missing": {f"{p}/{l}": v for (p, l), v in missing.items()},
            }
        )
    )
    return 0 if not missing else 1


if __name__ == "__main__":
    raise SystemExit(main())
