#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run Cargo and publish its binary without shell-specific command chaining."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def main():
    source, destination = map(Path, sys.argv[1:3])
    result = subprocess.run(sys.argv[3:], check=False)
    if result.returncode:
        return result.returncode
    # Copy to the output directory before replacing: Cargo's cache may live on
    # another filesystem, and a running Unix player must keep its old inode.
    fd, temporary = tempfile.mkstemp(prefix=destination.name + ".", dir=destination.parent)
    os.close(fd)
    try:
        shutil.copy2(source, temporary)
        os.replace(temporary, destination)
    finally:
        Path(temporary).unlink(missing_ok=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
