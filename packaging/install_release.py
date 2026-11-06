#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
import shutil
from pathlib import Path


PACKAGE_ROOT = Path(__file__).resolve().parent
ALLOWED_BINARIES = {"qwe", "qwed", "qwe-worker", "qwe-session-exec"}
ALLOWED_SERVICE_PREFIX = Path("share/qwe/service")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_package():
    manifest_path = PACKAGE_ROOT / "release.json"
    checksum_path = PACKAGE_ROOT / "SHA256SUMS"
    if not manifest_path.is_file() or not checksum_path.is_file():
        raise SystemExit("install.py must be run from a QWE release package")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema") != "qwe-release/v1":
        raise SystemExit(f"unsupported release manifest: {manifest.get('schema')}")

    expected = {}
    for line in checksum_path.read_text(encoding="utf-8").splitlines():
        digest, relative = line.split("  ", 1)
        relative_path = Path(relative)
        if relative_path.is_absolute() or ".." in relative_path.parts:
            raise SystemExit(f"unsafe checksum path: {relative}")
        if relative in expected:
            raise SystemExit(f"duplicate checksum path: {relative}")
        if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest):
            raise SystemExit(f"invalid SHA-256 digest for {relative}")
        expected[relative] = digest
    for relative, digest in expected.items():
        path = PACKAGE_ROOT / relative
        if not path.is_file() or sha256(path) != digest:
            raise SystemExit(f"release checksum mismatch: {relative}")
    return manifest


def replace_all(path, replacements):
    content = path.read_text(encoding="utf-8")
    for source, destination in replacements.items():
        if source not in content:
            raise SystemExit(f"{path} does not contain expected template {source}")
        content = content.replace(source, destination)
    path.write_text(content, encoding="utf-8")


def stage_copy(source, destination, mode):
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(f".{destination.name}.qwe-stage-{os.getpid()}")
    if temporary.exists():
        temporary.unlink()
    shutil.copy2(source, temporary)
    temporary.chmod(mode)
    os.replace(temporary, destination)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--prefix", type=Path, default=Path.home())
    parser.add_argument("--bin-dir", type=Path)
    parser.add_argument("--no-service-assets", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    manifest = verify_package()
    prefix = args.prefix.expanduser().resolve()
    if prefix == Path("/"):
        raise SystemExit("refusing to install the release directly into /")
    bin_dir = (args.bin_dir or prefix / ".local/bin").expanduser().resolve()
    share_dir = (prefix / "share/qwe").resolve()
    service_dir = share_dir / "service"
    if set(manifest.get("binaries", {})) != ALLOWED_BINARIES:
        raise SystemExit("release manifest has an unexpected binary set")
    service_assets = [Path(relative) for relative in manifest["service_assets"]]
    for relative in service_assets:
        if (
            relative.is_absolute()
            or ".." in relative.parts
            or ALLOWED_SERVICE_PREFIX not in relative.parents
        ):
            raise SystemExit(f"unsafe service asset path: {relative}")

    installed = []
    if not args.dry_run:
        bin_dir.mkdir(parents=True, exist_ok=True)
        for name in manifest["binaries"]:
            source = PACKAGE_ROOT / "bin" / name
            destination = bin_dir / name
            stage_copy(source, destination, 0o755)
            installed.append(str(destination))

        if not args.no_service_assets:
            for relative in manifest["service_assets"]:
                source = PACKAGE_ROOT / relative
                destination = share_dir / Path(relative).relative_to("share/qwe")
                stage_copy(
                    source,
                    destination,
                    0o755 if source.suffix == ".sh" else 0o644,
                )
            replace_all(
                service_dir / "systemd/qwe.service",
                {"%h/.local/bin/qwed": f'"{bin_dir / "qwed"}"'},
            )
            replace_all(
                service_dir / "launchd/com.qwe.qwed.plist",
                {
                    "REPOSITORY_ROOT/packaging/launchd/qwed-wrapper.sh": str(
                        bin_dir / "qwed-wrapper.sh"
                    )
                },
            )
            wrapper = PACKAGE_ROOT / "share/qwe/service/launchd/qwed-wrapper.sh"
            stage_copy(wrapper, bin_dir / "qwed-wrapper.sh", 0o755)
            replace_all(
                bin_dir / "qwed-wrapper.sh",
                {"$HOME/.local/bin/qwed": str(bin_dir / "qwed")},
            )

            system = manifest["target"].split("-", 1)[0]
            if system == "macos":
                agent_dir = prefix / "Library/LaunchAgents"
                stage_copy(
                    service_dir / "launchd/com.qwe.qwed.plist",
                    agent_dir / "com.qwe.qwed.plist",
                    0o644,
                )
            elif system == "linux":
                unit_dir = prefix / ".config/systemd/user"
                stage_copy(
                    service_dir / "systemd/qwe.service",
                    unit_dir / "qwe.service",
                    0o644,
                )

    print(
        json.dumps(
            {
                "case": "PACK-002",
                "version": manifest["version"],
                "target": manifest["target"],
                "prefix": str(prefix),
                "bin_dir": str(bin_dir),
                "service_assets": not args.no_service_assets,
                "dry_run": args.dry_run,
                "installed": installed,
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
