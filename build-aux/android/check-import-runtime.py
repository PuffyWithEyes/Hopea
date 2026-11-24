# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Ensure Gradle actually packaged the locally built security update."""
from pathlib import Path
import sys
import zipfile

RELEASE = "3.11.18-hopea1"


def check(runtime, libraries, assets, abi):
    target = runtime / "maven/com/chaquo/python/target" / RELEASE
    with zipfile.ZipFile(target / f"target-{RELEASE}-{abi}.zip") as expected, \
         zipfile.ZipFile(assets / f"stdlib-{abi}.imy") as extensions:
        for name in expected.namelist():
            if not name.endswith(".so"):
                continue
            filename = Path(name).name
            if name.startswith(f"jniLibs/{abi}/"):
                actual = (libraries / abi / filename).read_bytes()
            elif name.startswith(f"lib-dynload/{abi}/"):
                bootstrap = assets / "bootstrap-native" / abi / filename
                actual = bootstrap.read_bytes() if bootstrap.exists() else extensions.read(filename)
            else:
                continue
            if actual != expected.read(name):
                raise ValueError(f"Packaged Python native library differs from the security update: {filename}")
    with zipfile.ZipFile(target / f"target-{RELEASE}-stdlib-pyc.zip") as expected, \
         zipfile.ZipFile(assets / "stdlib-common.imy") as actual:
        for name in ("http/cookiejar.pyc", "ssl.pyc", "tarfile.pyc"):
            if actual.read(name) != expected.read(name):
                raise ValueError(f"Packaged Python module differs from the security update: {name}")


if __name__ == "__main__":
    check(*(Path(p) for p in sys.argv[1:4]), sys.argv[4])
    print("Packaged CPython native libraries and security modules: passed")
