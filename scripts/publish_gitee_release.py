#!/usr/bin/env python3
"""Create or update a Gitee release and upload the versioned QWE assets."""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any


API = "https://gitee.com/api/v5"


def request(
    method: str,
    url: str,
    *,
    token: str,
    form: dict[str, str] | None = None,
    body: bytes | None = None,
    content_type: str = "application/x-www-form-urlencoded",
) -> tuple[int, Any]:
    headers = {"User-Agent": "qwe-release-publisher"}
    if form is not None:
        body = urllib.parse.urlencode(form).encode()
    if body is not None:
        headers["Content-Type"] = content_type
    separator = "&" if "?" in url else "?"
    authenticated_url = (
        f"{url}{separator}{urllib.parse.urlencode({'access_token': token})}"
    )
    req = urllib.request.Request(
        authenticated_url, data=body, headers=headers, method=method
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as response:
            content = response.read()
            return response.status, json.loads(content) if content else None
    except urllib.error.HTTPError as failure:
        content = failure.read().decode(errors="replace")
        if failure.code == 404:
            return 404, None
        raise RuntimeError(
            f"Gitee API {method} {url} failed with {failure.code}: {content}"
        ) from failure


def upload_file(
    url: str,
    *,
    token: str,
    path: Path,
) -> dict[str, Any]:
    boundary = f"qwe-{uuid.uuid4().hex}"
    chunks = []
    chunks.append(f"--{boundary}\r\n".encode())
    chunks.append(
        b'Content-Disposition: form-data; name="access_token"\r\n\r\n'
        + token.encode()
        + b"\r\n"
    )
    chunks.append(f"--{boundary}\r\n".encode())
    chunks.append(
        (
            'Content-Disposition: form-data; name="file"; '
            f'filename="{path.name}"\r\n'
        ).encode()
    )
    chunks.append(b"Content-Type: application/octet-stream\r\n\r\n")
    chunks.append(path.read_bytes())
    chunks.append(f"\r\n--{boundary}--\r\n".encode())
    req = urllib.request.Request(
        url,
        data=b"".join(chunks),
        headers={
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            "User-Agent": "qwe-release-publisher",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as failure:
        content = failure.read().decode(errors="replace")
        raise RuntimeError(
            f"Gitee upload {path.name} failed with {failure.code}: {content}"
        ) from failure


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--checksum", type=Path, required=True)
    parser.add_argument("--notes", type=Path, required=True)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    if "/" not in args.repo:
        raise SystemExit("repo must use owner/name form")
    if not args.tag.startswith("v"):
        raise SystemExit("release tag must start with 'v'")
    for path in (args.archive, args.checksum, args.notes):
        if not path.is_file():
            raise SystemExit(f"release input is missing: {path}")

    body = args.notes.read_text(encoding="utf-8")
    payload = {
        "tag_name": args.tag,
        "name": f"QWE {args.tag.removeprefix('v')}",
        "body": body,
        "prerelease": "false",
        "target_commitish": args.target,
    }
    assets = [args.archive, args.checksum]
    if args.dry_run:
        print(
            json.dumps(
                {
                    "repo": args.repo,
                    "release": payload,
                    "assets": [path.name for path in assets],
                },
                separators=(",", ":"),
            )
        )
        return 0

    token = os.environ.get("GITEE_TOKEN")
    if not token:
        raise SystemExit("GITEE_TOKEN is required")
    release_url = f"{API}/repos/{args.repo}/releases"
    status, release = request(
        "GET",
        f"{release_url}/tags/{urllib.parse.quote(args.tag, safe='')}",
        token=token,
    )
    if status == 404:
        _, release = request(
            "POST", release_url, token=token, form=payload
        )
    else:
        release_id = release["id"]
        _, release = request(
            "PATCH",
            f"{release_url}/{release_id}",
            token=token,
            form={
                "tag_name": args.tag,
                "name": payload["name"],
                "body": body,
                "prerelease": "false",
            },
        )

    release_id = release["id"]
    attach_url = f"{release_url}/{release_id}/attach_files"
    _, existing = request("GET", attach_url, token=token)
    wanted = {path.name for path in assets}
    for asset in existing or []:
        if asset.get("name") in wanted:
            request(
                "DELETE",
                f"{attach_url}/{asset['id']}",
                token=token,
            )
    uploaded = []
    for path in assets:
        asset = upload_file(attach_url, token=token, path=path)
        uploaded.append(
            {
                "name": asset.get("name", path.name),
                "size": asset.get("size", path.stat().st_size),
                "url": asset.get("browser_download_url"),
            }
        )
    print(
        json.dumps(
            {
                "tag": args.tag,
                "release_url": release.get("html_url"),
                "assets": uploaded,
            },
            separators=(",", ":"),
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
