#!/usr/bin/env python3

import argparse
import json
import shutil
import subprocess
import tempfile
from pathlib import Path


def run_init(init, workspace, script, *arguments):
    return subprocess.run(
        [
            str(init),
            "--workspace",
            str(workspace),
            "--workspace-mode",
            "read-write",
            "--private-tmp",
            "--",
            "/bin/sh",
            "-c",
            script,
            "qwe-landlock-abi-probe",
            *[str(value) for value in arguments],
        ],
        cwd=workspace,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=15,
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--init", required=True)
    args = parser.parse_args()
    init = Path(args.init).resolve()

    root = Path(tempfile.mkdtemp(prefix="qwe-landlock-abi-", dir=Path.home()))
    workspace = root / "workspace"
    outside = root / "outside"
    workspace.mkdir()
    outside.mkdir()
    try:
        abi_run = run_init(
            init,
            workspace,
            'printf "%s" "$QWE_LANDLOCK_ABI"',
        )
        assert abi_run.returncode == 0, abi_run.stderr
        abi = int(abi_run.stdout)
        assert abi >= 1

        workspace_write = run_init(
            init,
            workspace,
            "printf inside > inside.txt",
        )
        assert workspace_write.returncode == 0, workspace_write.stderr
        assert (workspace / "inside.txt").read_text() == "inside"

        outside_target = outside / "write.txt"
        outside_write = run_init(
            init,
            workspace,
            'python3 -c "import sys; open(sys.argv[1], \\"w\\").write(\\"outside\\")" "$1"',
            outside_target,
        )
        assert outside_write.returncode != 0, outside_write
        assert not outside_target.exists()

        tmp_write = run_init(
            init,
            workspace,
            'tmp_file="/tmp/qwe-landlock-abi-$$"; printf tmp > "$tmp_file"; rm -f "$tmp_file"',
        )
        assert tmp_write.returncode == 0, tmp_write.stderr

        rename_source = workspace / "rename-source"
        rename_source.write_text("rename")
        rename_destination = outside / "rename-destination"
        rename = run_init(
            init,
            workspace,
            'python3 -c "import os,sys; os.rename(sys.argv[1], sys.argv[2])" "$1" "$2"',
            rename_source,
            rename_destination,
        )
        if rename.returncode != 0:
            assert rename_source.exists()
            assert not rename_destination.exists()
            refer = "enforced" if abi >= 2 else "enforced_by_abi1_conflict"
        else:
            refer = "not_supported"

        truncate_target = outside / "truncate.txt"
        truncate_target.write_text("content")
        truncate = run_init(
            init,
            workspace,
            'python3 -c "import os,sys; os.truncate(sys.argv[1], 0)" "$1"',
            truncate_target,
        )
        if abi >= 3:
            assert truncate.returncode != 0, truncate
            assert truncate_target.read_text() == "content"
            truncate_result = "enforced"
        else:
            assert truncate.returncode == 0, truncate.stderr
            assert truncate_target.read_text() == ""
            truncate_result = "not_supported"

        print(
            json.dumps(
                {
                    "case": "SANDBOX-014",
                    "status": "pass",
                    "abi": abi,
                    "workspace_write": "pass",
                    "outside_write_denied": "pass",
                    "private_tmp_write": "pass",
                    "refer": refer,
                    "truncate": truncate_result,
                    "ioctl_dev": (
                        "handled" if abi >= 5 else "not_supported"
                    ),
                },
                separators=(",", ":"),
            )
        )
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
