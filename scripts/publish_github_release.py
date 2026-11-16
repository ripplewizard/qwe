#!/usr/bin/env python3
"""Create or update a GitHub release and upload the versioned QWE assets."""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


def request(
    method: str,
    url: str,
    *,
    token: str,
    payload: dict[str, Any] | None = None,
    body: bytes | None = None,
    content_type: str = "application/json",
) -> tuple[int, dict[str, Any] | None]:
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "qwe-release-publisher",
    }
    data = body
    if payload is not None:
        data = json.dumps(payload, separators=(",", ":")).encode()
    if data is not None:
        headers["Content-Type"] = content_type
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=60) as response:
            content = response.read()
            return response.status, json.loads(content) if content else None
    except urllib.error.HTTPError as failure:
        content = failure.read().decode(errors="replace")
        if failure.code == 404:
            return 404, None
        raise RuntimeError(
            f"GitHub API {method} {url} failed with {failure.code}: {content}"
        ) from failure


def release_payload(tag: str, notes: str) -> dict[str, Any]:
    version = tag.removeprefix("v")
    return {
        "tag_name": tag,
        "name": f"QWE {version}",
        "body": notes,
        "draft": False,
        "prerelease": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--checksum", type=Path, required=True)
    parser.add_argument("--notes", type=Path, required=True)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    if not args.tag.startswith("v"):
        raise SystemExit("release tag must start with 'v'")
    for path in (args.archive, args.checksum, args.notes):
        if not path.is_file():
            raise SystemExit(f"release input is missing: {path}")

    checksum_text = args.checksum.read_text(encoding="utf-8")
    checksum_value = checksum_text.split(maxsplit=1)[0]
    notes = (
        args.notes.read_text(encoding="utf-8").rstrip()
        + "\n\n## Published archive\n\n"
        + f"`{args.archive.name}` SHA-256: `{checksum_value}`\n"
    )
    assets = [
        (args.archive.name, args.archive.read_bytes()),
        (args.checksum.name, args.checksum.read_bytes()),
    ]
    payload = release_payload(args.tag, notes)
    if args.dry_run:
        print(
            json.dumps(
                {
                    "repo": args.repo,
                    "tag": args.tag,
                    "release": payload,
                    "assets": [name for name, _ in assets],
                },
                separators=(",", ":"),
            )
        )
        return 0

    token = os.environ.get("GITHUB_TOKEN")
    if not token:
        raise SystemExit("GITHUB_TOKEN is required")
    api = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
    tag_path = urllib.parse.quote(args.tag, safe="")
    release_url = f"{api}/repos/{args.repo}/releases"
    status, release = request(
        "GET", f"{release_url}/tags/{tag_path}", token=token
    )
    if status == 404:
        _, release = request(
            "POST", release_url, token=token, payload=payload
        )
    else:
        assert release is not None
        _, release = request(
            "PATCH",
            f"{release_url}/{release['id']}",
            token=token,
            payload=payload,
        )
    assert release is not None

    _, existing = request(
        "GET",
        f"{release_url}/{release['id']}/assets?per_page=100",
        token=token,
    )
    wanted = {name for name, _ in assets}
    for asset in existing or []:
        if asset["name"] in wanted:
            request(
                "DELETE",
                f"{api}/repos/{args.repo}/releases/assets/{asset['id']}",
                token=token,
            )

    upload_base = release["upload_url"].split("{", 1)[0]
    uploaded = []
    for name, content in assets:
        upload_url = f"{upload_base}?{urllib.parse.urlencode({'name': name})}"
        _, asset = request(
            "POST",
            upload_url,
            token=token,
            body=content,
            content_type="application/octet-stream",
        )
        assert asset is not None
        uploaded.append(
            {
                "name": asset["name"],
                "size": asset["size"],
                "url": asset["browser_download_url"],
            }
        )
    print(
        json.dumps(
            {
                "tag": args.tag,
                "release_url": release["html_url"],
                "assets": uploaded,
            },
            separators=(",", ":"),
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
