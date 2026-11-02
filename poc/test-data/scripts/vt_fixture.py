#!/usr/bin/env python3
import os
import sys


output = "".join(
    [
        "\x1b]0;QWE_VT_TEST\x07",
        "\x1b[2J\x1b[H",
        "QWE_BASE\r\n",
        "\x1b[2;1H\x1b[31mRED\x1b[0m \x1b[1;34mBLUE\x1b[0m",
        "\x1b[3;1HABCDEF\x1b[3;2H\x1b[K",
        "\x1b[4;1H中文 😀 e\u0301",
        "\x1b[6;4H\x1b[?25lQWE_END",
    ]
)
os.write(sys.stdout.fileno(), output.encode("utf-8"))
