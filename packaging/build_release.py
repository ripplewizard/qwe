#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
from datetime import UTC, datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BINARIES = ("qwe", "qwed", "qwe-worker", "qwe-session-exec")


def run(arguments):
    return subprocess.run(
        [str(value) for value in arguments],
        cwd=ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    ).stdout.strip()


def normalized_target():
    machine = platform.machine().lower()
    architecture = {
        "aarch64": "arm64",
        "arm64": "arm64",
        "x86_64": "x86_64",
        "amd64": "x86_64",
    }.get(machine, machine)
    system = platform.system()
    if system == "Darwin":
        return f"macos-{architecture}"
    if system == "Linux":
        return f"linux-{architecture}"
    raise SystemExit(f"unsupported release platform: {system} {machine}")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def source_commit():
    commit = run(["git", "rev-parse", "--short=12", "HEAD"])
    dirty = bool(run(["git", "status", "--porcelain", "--untracked-files=normal"]))
    return commit, dirty


def sign_binary(path, identity):
    if platform.system() != "Darwin":
        raise SystemExit("--sign-identity is only supported on macOS")
    run(
        [
            "codesign",
            "--force",
            "--options",
            "runtime",
            "--timestamp",
            "--sign",
            identity,
            path,
        ]
    )
    run(["codesign", "--verify", "--strict", "--verbose=2", path])


def copy_file(source, destination, mode):
    if not source.is_file():
        raise SystemExit(f"required release input is missing: {source}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)
    destination.chmod(mode)


def write_checksums(package_root):
    files = sorted(
        path
        for path in package_root.rglob("*")
        if path.is_file() and path.name != "SHA256SUMS"
    )
    lines = [
        f"{sha256(path)}  {path.relative_to(package_root).as_posix()}\n"
        for path in files
    ]
    (package_root / "SHA256SUMS").write_text("".join(lines), encoding="utf-8")


def write_archive(package_root, archive_path):
    with tarfile.open(archive_path, "w:gz", format=tarfile.PAX_FORMAT) as archive:
        for path in sorted(package_root.rglob("*")):
            relative = path.relative_to(package_root.parent)
            info = archive.gettarinfo(str(path), arcname=relative.as_posix())
            info.uid = 0
            info.gid = 0
            info.uname = ""
            info.gname = ""
            info.mtime = 0
            if path.is_file():
                with path.open("rb") as stream:
                    archive.addfile(info, stream)
            else:
                archive.addfile(info)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", type=Path, default=ROOT / "target/release")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "dist")
    parser.add_argument("--version")
    parser.add_argument("--target")
    parser.add_argument("--sign-identity")
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()

    with (ROOT / "Cargo.toml").open("rb") as stream:
        workspace = tomllib.load(stream)
    version = args.version or workspace["workspace"]["package"]["version"]
    target = args.target or normalized_target()
    commit, dirty = source_commit()
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    package_name = f"qwe-{version}-{target}"

    final_root = output_dir / package_name
    archive_path = output_dir / f"{package_name}.tar.gz"
    if final_root.exists() or archive_path.exists():
        if not args.force:
            raise SystemExit(
                f"release output already exists: {final_root} or {archive_path}; "
                "pass --force to replace generated output"
            )

    with tempfile.TemporaryDirectory(prefix="qwe-release-", dir=output_dir) as scratch:
        scratch = Path(scratch)
        package_root = scratch / package_name
        staged_archive = scratch / f"{package_name}.tar.gz"
        bin_dir = package_root / "bin"
        service_dir = package_root / "share/qwe/service"
        bin_dir.mkdir(parents=True)
        service_dir.mkdir(parents=True)

        binary_records = {}
        for name in BINARIES:
            source = args.bin_dir / name
            destination = bin_dir / name
            copy_file(source, destination, 0o755)
            if args.sign_identity:
                sign_binary(destination, args.sign_identity)
            binary_records[name] = {
                "bytes": destination.stat().st_size,
                "sha256": sha256(destination),
            }

        copy_file(ROOT / "packaging/install_release.py", package_root / "install.py", 0o755)
        copy_file(
            ROOT / "packaging/RELEASE_README.md",
            package_root / "README.md",
            0o644,
        )
        copy_file(ROOT / "packaging/README.md", package_root / "INSTALL.md", 0o644)
        copy_file(
            ROOT / "packaging/systemd/qwe.service",
            service_dir / "systemd/qwe.service",
            0o644,
        )
        copy_file(
            ROOT / "packaging/launchd/com.qwe.qwed.plist",
            service_dir / "launchd/com.qwe.qwed.plist",
            0o644,
        )
        copy_file(
            ROOT / "packaging/launchd/qwed-wrapper.sh",
            service_dir / "launchd/qwed-wrapper.sh",
            0o755,
        )

        version_report = json.loads(run([bin_dir / "qwe"]))
        if version_report.get("version") != version:
            raise SystemExit(
                f"binary version {version_report.get('version')} does not match "
                f"release version {version}"
            )
        for name in ("qwed", "qwe-worker", "qwe-session-exec"):
            report = run([bin_dir / name, "--version"])
            if f" {version}" not in report:
                raise SystemExit(
                    f"{name} version report does not match release version {version}: "
                    f"{report}"
                )

        manifest = {
            "schema": "qwe-release/v1",
            "product": "qwe",
            "version": version,
            "protocol_version": version_report["protocol"],
            "target": target,
            "source_commit": commit,
            "source_dirty": dirty,
            "built_at_utc": datetime.now(UTC).isoformat(),
            "signed_identity": args.sign_identity,
            "binaries": binary_records,
            "service_assets": [
                "share/qwe/service/systemd/qwe.service",
                "share/qwe/service/launchd/com.qwe.qwed.plist",
                "share/qwe/service/launchd/qwed-wrapper.sh",
            ],
        }
        (package_root / "release.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        write_checksums(package_root)
        write_archive(package_root, staged_archive)

        if final_root.exists():
            shutil.rmtree(final_root)
        if archive_path.exists():
            archive_path.unlink()
        shutil.copytree(package_root, final_root)
        os.replace(staged_archive, archive_path)

    print(
        json.dumps(
            {
                "case": "PACK-001",
                "package": str(final_root),
                "archive": str(archive_path),
                "archive_sha256": sha256(archive_path),
                "version": version,
                "target": target,
                "source_commit": commit,
                "source_dirty": dirty,
                "signed_identity": args.sign_identity,
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
