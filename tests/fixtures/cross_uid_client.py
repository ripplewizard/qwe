#!/usr/bin/python3

import json
import os
import socket
from pathlib import Path


SOCKET_PATH_FILE = Path("/tmp/qwe-cross-uid-client.socket")


def main():
    socket_path = SOCKET_PATH_FILE.read_text(encoding="utf-8").strip()
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
            stream.settimeout(2)
            stream.connect(socket_path)
    except PermissionError as failure:
        print(
            json.dumps(
                {
                    "result": "connect_denied",
                    "error": str(failure),
                },
                separators=(",", ":"),
            )
        )
        return
    print(
        json.dumps(
            {"result": "connect_allowed", "uid": os.getuid()},
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
