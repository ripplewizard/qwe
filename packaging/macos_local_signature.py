#!/usr/bin/env python3

import argparse
import json
import os
import re
import secrets
import shlex
import subprocess
import tempfile
from datetime import UTC, datetime
from pathlib import Path


STATE_HOME = Path(
    os.environ.get(
        "QWE_LOCAL_SIGNING_HOME",
        Path.home() / "Library/Application Support/QWE/local-signing",
    )
).expanduser()
STATE_FILE = STATE_HOME / "identity.json"
KEYCHAIN = STATE_HOME / "QWE-Local-Signing.keychain-db"
CERTIFICATE = STATE_HOME / "QWE-Local-Signing.pem"


def run(arguments, *, check=True, capture=True):
    return subprocess.run(
        [str(value) for value in arguments],
        check=check,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )


def require_macos_tools():
    for command in ("openssl", "security", "codesign"):
        if subprocess.run(
            ["/usr/bin/which", command],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        ).returncode:
            raise SystemExit(f"{command} is required")


def load_state(required=True):
    if not STATE_FILE.is_file():
        if required:
            raise SystemExit(
                "local signing identity is not configured; "
                "run packaging/macos_local_signature.py create"
            )
        return None
    state = json.loads(STATE_FILE.read_text(encoding="utf-8"))
    if not Path(state["keychain"]).is_file():
        raise SystemExit(f"local signing keychain is missing: {state['keychain']}")
    if not Path(state["certificate"]).is_file():
        raise SystemExit(f"local signing certificate is missing: {state['certificate']}")
    return state


def identity_present(state):
    result = run(
        [
            "security",
            "find-identity",
            "-v",
            "-p",
            "codesigning",
            state["keychain"],
        ],
        check=False,
    )
    return state["identity"] in result.stdout


def create_identity(args):
    require_macos_tools()
    if STATE_FILE.exists():
        raise SystemExit(
            f"local signing identity already exists at {STATE_FILE}; "
            "run the remove command first"
        )

    STATE_HOME.mkdir(parents=True, exist_ok=True)
    STATE_HOME.chmod(0o700)
    keychain_password = secrets.token_urlsafe(32)
    p12_password = secrets.token_urlsafe(24)
    subject = "QWE Local Release Signing"
    certificate = None
    with tempfile.TemporaryDirectory(
        prefix=".create-", dir=STATE_HOME
    ) as temporary:
        temporary = Path(temporary)
        private_key = temporary / "identity.key"
        exported_p12 = temporary / "identity.p12"
        created_certificate = temporary / "identity.pem"
        run(
            [
                "openssl",
                "req",
                "-x509",
                "-newkey",
                "rsa:3072",
                "-nodes",
                "-keyout",
                private_key,
                "-out",
                created_certificate,
                "-days",
                str(args.days),
                "-subj",
                f"/CN={subject}/O=QWE Local Development",
                "-addext",
                "basicConstraints=critical,CA:false",
                "-addext",
                "keyUsage=critical,digitalSignature",
                "-addext",
                "extendedKeyUsage=critical,codeSigning",
                "-addext",
                "subjectKeyIdentifier=hash",
                "-addext",
                "authorityKeyIdentifier=keyid,issuer",
            ]
        )
        run(
            [
                "openssl",
                "pkcs12",
                "-export",
                "-out",
                exported_p12,
                "-inkey",
                private_key,
                "-in",
                created_certificate,
                "-passout",
                f"pass:{p12_password}",
            ]
        )

        try:
            run(["security", "create-keychain", "-p", keychain_password, KEYCHAIN])
            KEYCHAIN.chmod(0o600)
            run(["security", "set-keychain-settings", "-lut", "21600", KEYCHAIN])
            run(["security", "unlock-keychain", "-p", keychain_password, KEYCHAIN])
            run(
                [
                    "security",
                    "import",
                    exported_p12,
                    "-k",
                    KEYCHAIN,
                    "-P",
                    p12_password,
                    "-T",
                    "/usr/bin/codesign",
                ]
            )
            run(
                [
                    "security",
                    "set-key-partition-list",
                    "-S",
                    "apple-tool:,apple:",
                    "-s",
                    "-k",
                    keychain_password,
                    KEYCHAIN,
                ]
            )
            certificate = STATE_HOME / CERTIFICATE.name
            certificate.write_bytes(created_certificate.read_bytes())
            certificate.chmod(0o644)
            run(
                [
                    "security",
                    "add-trusted-cert",
                    "-r",
                    "trustRoot",
                    "-p",
                    "codeSign",
                    "-k",
                    KEYCHAIN,
                    certificate,
                ]
            )

            identities = run(
                [
                    "security",
                    "find-identity",
                    "-v",
                    "-p",
                    "codesigning",
                    KEYCHAIN,
                ]
            ).stdout
            match = re.search(r'^\s*1\)\s+([0-9A-F]+)\s+"', identities, re.MULTILINE)
            if match is None:
                raise SystemExit(f"could not resolve local signing identity:\n{identities}")
            state = {
                "schema": "qwe-local-signature/v1",
                "subject": subject,
                "identity": match.group(1),
                "keychain": str(KEYCHAIN),
                "keychain_password": keychain_password,
                "certificate": str(certificate),
                "created_at_utc": datetime.now(UTC).isoformat(),
                "expires_after_days": args.days,
                "public_trust": False,
            }
            STATE_FILE.write_text(
                json.dumps(state, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            STATE_FILE.chmod(0o600)
        except Exception:
            if certificate is not None and certificate.exists():
                run(
                    ["security", "remove-trusted-cert", certificate],
                    check=False,
                )
                certificate.unlink(missing_ok=True)
            run(["security", "delete-keychain", KEYCHAIN], check=False)
            raise

    state = load_state()
    print(
        json.dumps(
            {
                "case": "MAC-001",
                "configured": identity_present(state),
                "subject": state["subject"],
                "identity": state["identity"],
                "keychain": state["keychain"],
                "certificate": state["certificate"],
                "expires_after_days": state["expires_after_days"],
                "public_trust": False,
            },
            sort_keys=True,
            separators=(",", ":"),
        )
    )


def remove_identity():
    state = load_state(required=False)
    if state is None:
        raise SystemExit("local signing identity is not configured")
    certificate = Path(state["certificate"])
    keychain = Path(state["keychain"])
    if certificate.exists():
        run(["security", "remove-trusted-cert", certificate], check=False)
    if keychain.exists():
        run(["security", "delete-keychain", keychain], check=False)
    certificate.unlink(missing_ok=True)
    STATE_FILE.unlink(missing_ok=True)
    keychain.unlink(missing_ok=True)
    try:
        STATE_HOME.rmdir()
    except OSError:
        pass
    print(
        json.dumps(
            {
                "case": "MAC-001",
                "local_signing_identity": "removed",
                "public_trust": False,
            },
            separators=(",", ":"),
        )
    )


def temporarily_add_keychain(keychain):
    original = shlex.split(run(["security", "list-keychains", "-d", "user"]).stdout)

    def restore():
        if original:
            run(["security", "list-keychains", "-d", "user", "-s", *original])

    run(["security", "list-keychains", "-d", "user", "-s", keychain, *original])
    return restore


def sign_files(paths):
    require_macos_tools()
    state = load_state()
    if not identity_present(state):
        raise SystemExit(
            "local signing identity is unavailable; recreate or repair the "
            "local signing keychain"
        )
    run(
        [
            "security",
            "unlock-keychain",
            "-p",
            state["keychain_password"],
            state["keychain"],
        ]
    )
    restore = temporarily_add_keychain(state["keychain"])
    signed = []
    try:
        for path in paths:
            path = Path(path)
            if not path.is_file():
                raise SystemExit(f"binary to sign is missing: {path}")
            run(
                [
                    "codesign",
                    "--force",
                    "--options",
                    "runtime",
                    "--timestamp=none",
                    "--sign",
                    state["identity"],
                    path,
                ]
            )
            run(["codesign", "--verify", "--strict", "--verbose=2", path])
            signed.append(str(path))
    finally:
        restore()
    print(
        json.dumps(
            {
                "case": "MAC-001",
                "signing_mode": "self-signed",
                "identity": state["subject"],
                "public_trust": False,
                "signed": signed,
            },
            separators=(",", ":"),
        )
    )


def status_identity():
    state = load_state(required=False)
    if state is None:
        print(
            json.dumps(
                {
                    "case": "MAC-001",
                    "configured": False,
                    "public_trust": False,
                },
                separators=(",", ":"),
            )
        )
        return
    print(
        json.dumps(
            {
                "case": "MAC-001",
                "configured": identity_present(state),
                "subject": state["subject"],
                "identity": state["identity"],
                "keychain": state["keychain"],
                "public_trust": False,
            },
            separators=(",", ":"),
        )
    )


def main():
    parser = argparse.ArgumentParser()
    subcommands = parser.add_subparsers(dest="command", required=True)
    create = subcommands.add_parser("create")
    create.add_argument("--days", type=int, default=365)
    create.set_defaults(handler=create_identity)
    remove = subcommands.add_parser("remove")
    remove.set_defaults(handler=lambda _: remove_identity())
    sign = subcommands.add_parser("sign")
    sign.add_argument("files", nargs="+", type=Path)
    sign.set_defaults(handler=lambda args: sign_files(args.files))
    status = subcommands.add_parser("status")
    status.set_defaults(handler=lambda _: status_identity())
    args = parser.parse_args()
    args.handler(args)


if __name__ == "__main__":
    main()
