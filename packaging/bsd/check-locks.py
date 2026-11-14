#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Fail before compilation if the source or temporary filesystem cannot lock."""
import fcntl
from pathlib import Path
import subprocess
import sys
import tempfile

CHILD = """
import errno, fcntl, sys
with open(sys.argv[1], 'r+b') as handle:
    try:
        getattr(fcntl, sys.argv[2])(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as error:
        if error.errno not in (errno.EACCES, errno.EAGAIN):
            raise
        blocked = True
    else:
        blocked = False
    assert blocked == (sys.argv[3] == 'blocked'), 'Lock exclusion failed'
"""


def main():
    for directory in (Path.cwd(), Path(tempfile.gettempdir())):
        with tempfile.NamedTemporaryFile(prefix="hopea-lock-", dir=directory) as handle:
            for method in ("flock", "lockf"):
                lock = getattr(fcntl, method)
                lock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                command = [sys.executable, "-c", CHILD, handle.name, method]
                subprocess.run([*command, "blocked"], check=True, timeout=15)
                lock(handle, fcntl.LOCK_UN)
                subprocess.run([*command, "available"], check=True, timeout=15)
        print(f"File locking verified: {directory}", flush=True)


if __name__ == "__main__":
    main()
