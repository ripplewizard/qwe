#!/usr/bin/env python3

import json
import plistlib
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def main():
    unit = (ROOT / "packaging/systemd/qwe.service").read_text(encoding="utf-8")
    for expected in (
        "ExecStart=%h/.local/bin/qwed",
        "Restart=on-failure",
        "RuntimeDirectory=qwe",
        "StateDirectory=qwe",
        "UMask=0077",
        "KillMode=process",
        "NoNewPrivileges=true",
    ):
        assert expected in unit, expected

    with (ROOT / "packaging/launchd/com.qwe.qwed.plist").open("rb") as stream:
        launchd = plistlib.load(stream)
    assert launchd["Label"] == "com.qwe.qwed"
    assert launchd["RunAtLoad"] is True
    assert launchd["KeepAlive"] == {"SuccessfulExit": False}
    assert launchd["ProcessType"] == "Interactive"
    assert launchd["ProgramArguments"][1].endswith(
        "/packaging/launchd/qwed-wrapper.sh"
    )

    wrapper = (ROOT / "packaging/launchd/qwed-wrapper.sh").read_text(
        encoding="utf-8"
    )
    assert 'umask 077' in wrapper
    assert '--keychain-service-prefix qwe' in wrapper
    assert "qwe.sqlite" in wrapper

    assert re.search(r"^KillMode=process$", unit, flags=re.MULTILINE)
    print(
        json.dumps(
            {
                "case": "SVC-001",
                "systemd_unit": "pass",
                "launchd_plist": "pass",
                "credential_boundary_documented": "pass",
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
