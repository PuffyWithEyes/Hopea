# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Reject Android audio bundles with missing plugins or native dependencies."""
import re
import subprocess
import sys
from pathlib import Path

root, readelf = Path(sys.argv[1]), sys.argv[2]
required = {
    "libgstcoreelements.so", "libgstplayback.so", "libgsttypefindfunctions.so",
    "libgstaudioconvert.so", "libgstaudioresample.so", "libgstautodetect.so",
    "libgstopensles.so", "libgstlibav.so", "libgstsoup.so", "libsoup-3.0.so",
    "libgstlevel.so", "libgstisomp4.so", "libgstaudioparsers.so", "libgstflac.so",
}
libraries = {path.name: path for path in root.glob("*.so")}
missing = required - libraries.keys()
if missing:
    raise SystemExit(f"Missing Android audio libraries: {sorted(missing)}")

# Public NDK libraries provided by the OS, not copied into the APK.
system = {
    "libc.so", "libm.so", "libdl.so", "liblog.so", "libz.so", "libandroid.so",
    "libOpenSLES.so", "libEGL.so", "libGLESv1_CM.so", "libGLESv2.so",
    "libGLESv3.so", "libvulkan.so", "libjnigraphics.so", "libmediandk.so",
    "libaaudio.so", "libcamera2ndk.so", "libnativewindow.so",
}
for name, path in sorted(libraries.items()):
    dynamic = subprocess.check_output([readelf, "-d", str(path)], text=True)
    needed = set(re.findall(r"\(NEEDED\).*\[([^]]+)\]", dynamic))
    absent = needed - libraries.keys() - system
    if absent:
        raise SystemExit(f"{name}: unpackaged dependencies: {sorted(absent)}")

# The shared soup plugin loads this name dynamically, outside DT_NEEDED.
if b"libsoup-3.0.so\0" not in libraries["libgstsoup.so"].read_bytes():
    raise SystemExit("The HTTP plugin does not use Android's libsoup filename")
print("Android audio plugins, HTTP loader and native dependency closure: passed")
